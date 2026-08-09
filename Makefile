# AgentLens 根 Makefile：Rust + 前端统一入口
# 依赖：cargo、rustup 组件 rustfmt/clippy、npm、cargo-tauri（cargo install tauri-cli --locked）
# 打包额外依赖：rustup target x86_64-unknown-linux-musl / aarch64-unknown-linux-musl、
#              musl-gcc（x86_64）、aarch64 musl C 交叉编译器（见 dist-collector-aarch64）。

SHELL := /bin/bash
.SHELLFLAGS := -eu -o pipefail -c
FRONTEND := frontend
NPM := npm --prefix $(FRONTEND)

# ---------------------------------------------------------------------------
# 版本单一事实源：根 Cargo.toml 的 [workspace.package].version。
# 子 crate 与 src-tauri 全部 `version.workspace = true`，tauri.conf.json 已删除
# 硬编码 version（Tauri 缺省回落到 Cargo 包版本），所以 deb 文件名、二进制
# `--version`、release workflow 三处都由这一个值派生，不存在第二处需要同步。
# ---------------------------------------------------------------------------
VERSION := $(shell sed -n '/^\[workspace\.package\]/,/^\[/ s/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' Cargo.toml)
HOST_TRIPLE := $(shell rustc -vV | sed -n 's/^host: //p')

MUSL_X86_TARGET := x86_64-unknown-linux-musl
MUSL_ARM_TARGET := aarch64-unknown-linux-musl
COLLECTOR_X86 := agentlens-collector-$(MUSL_X86_TARGET)
COLLECTOR_ARM := agentlens-collector-$(MUSL_ARM_TARGET)
ASKPASS_BIN := agentlens-askpass

# tauri 的 externalBin 约定：源文件名必须以 `-<构建 target triple>` 结尾，bundler 复制到
# /usr/bin 时会把这段后缀去掉。所以 stage 目录里放带后缀的副本，安装后的名字正好是
# src-tauri/src/state.rs 与 transport/ssh.rs 期望的 `agentlens-collector-<arch>-unknown-linux-musl`。
STAGE_DIR := target/dist/binaries
TOOLCHAIN_DIR := target/dist/toolchain
DIST_DIR := artifacts/dist
BUNDLE_CONFIG := src-tauri/tauri.bundle.linux.json
DEB_DIR := target/release/bundle/deb

COLLECTOR_X86_BUILT := target/$(MUSL_X86_TARGET)/release/agentlens-collector
COLLECTOR_ARM_BUILT := target/$(MUSL_ARM_TARGET)/release/agentlens-collector
ASKPASS_BUILT := target/release/$(ASKPASS_BIN)

# aarch64 musl 的 C 交叉编译器：rusqlite 的 bundled feature 要编译 sqlite3.c，
# 必须有 musl ABI 的 aarch64 cc。优先用系统 aarch64-linux-musl-gcc，否则退到
# `zig cc -target aarch64-linux-musl`（zig 自带 musl sysroot）。
AARCH64_MUSL_CC ?= $(shell command -v aarch64-linux-musl-gcc 2>/dev/null)
ZIG := $(shell command -v zig 2>/dev/null)
# 缺 aarch64 工具链时 `dist` 只告警并产出单架构包；`dist-all` 则硬失败。
DIST_REQUIRE_AARCH64 ?= 0

# ---------------------------------------------------------------------------
# 格式化分工：rustfmt 管 Rust，prettier 管 frontend/src 下的 TS/TSX，oxfmt 管
# 剩下的 YAML / JSON / Markdown。三者的作用域由**传给工具的路径**保证互斥——
# oxfmt 只接收下面这批 glob，永远拿不到 .ts/.tsx，因此不会和 frontend/.prettierrc.json
# （singleQuote/no-semi/printWidth=100）打架；.oxfmtignore 再排除生成物与他人持有的文件。
# oxfmt 默认只读 .gitignore/.prettierignore，不认 .oxfmtignore 这个名字，必须显式 --ignore-path。
# ---------------------------------------------------------------------------
OXFMT := $(shell command -v oxfmt 2>/dev/null)
OXFMT_GLOBS := '**/*.yml' '**/*.yaml' '**/*.json' '**/*.jsonc' '**/*.md'
OXFMT_ARGS := --ignore-path .oxfmtignore --no-error-on-unmatched-pattern
OXFMT_HINT := 未找到 oxfmt，跳过 YAML/JSON/Markdown 格式化（安装：mise use -g oxfmt）

# ---------------------------------------------------------------------------
# 覆盖率：cargo-llvm-cov 跑一次插桩测试，产出人读摘要与 lcov 两份报告。
# 门禁读的是 lcov.info 的 LF/LH（Codecov 消费的同一份数据），所以本地门禁数字与
# Codecov 面板数字同源，不存在「本地过了线上红」的漂移。
#
# COVERAGE_MIN 的取法：2026-08-06 在 Linux 以 `cargo llvm-cov --workspace` 实测
# workspace 行覆盖率 91.63%（基线 15894/17346；门禁复验 15895/17346），据此把硬地板
# 定在 90，留 1.63pp 余量。复验分母稳定、命中仅差 1 行（约 0.006pp）；历史上分母不同的
# 2.6pp 读数来自插桩范围或代码状态不同，不是 runner 随机抖动。后续仍按棘轮逐档上调。
# 注意 11 个 #[ignore] 用例（真实大库 / 外部二进制 / 真实钥匙串）不参与统计，天花板低于纸面。
# ---------------------------------------------------------------------------
COVERAGE_DIR := artifacts/coverage
COVERAGE_LCOV := $(COVERAGE_DIR)/lcov.info
COVERAGE_SUMMARY := $(COVERAGE_DIR)/summary.txt
# 用 := 压过同名环境变量，避免硬地板被静默降低；命令行 COVERAGE_MIN=... 仍保持最高优先级。
COVERAGE_MIN := 90

# 钩子安装方式：auto = 有 pre-commit 就用框架，否则退化为纯 .git/hooks 脚本；
# plain = 强制走纯脚本（用于验证退化路径本身可用）。
HOOKS_MODE ?= auto

# ---------------------------------------------------------------------------
# AWS CodeBuild 验证通道（us-west-2）
#
# 定位：CodeBuild 是**验证场**，不是 GitHub Actions 的替代品。三平台产物先在这里
# 跑绿，之后 GitHub workflow 才照抄已证明的步骤——所以 buildspec 里的每一步都是
# 对本地同名 make / npm / cargo 目标的直接调用，翻译成 `run:` 是一对一的。
#
# ★ 环境陷阱（踩过一次就够）：本机 shell 导出了 AWS_REGION=cn-northwest-1 与
#   AWS_DEFAULT_REGION=cn-northwest-1（遗留的中国区 profile）。不带 --region 时
#   CLI 会去**中国分区**的端点，用 us profile 的凭据必然报
#   `InvalidClientTokenId`，而不是权限错误——极易误判成授权问题。
#   下面把 --region / --profile 固化进 $(AWS) 变量，凡走这些目标就不可能漏传。
#
# ★ 源类型是 S3 而不是 GitHub：本仓库**还没有 git remote**。所以打包工作树 →
#   上传 zip → CodeBuild 从 zip 构建，这正是「先验证、后建 GitHub workflow」
#   这个顺序能成立的前提。
#
# 账号 ID 不写在本文件里：默认由 `aws sts get-caller-identity` 在运行期取，桶名再从
# 账号派生，所以换账号通常什么都不用改——换 AWS_PROFILE 就够了。要显式指定：
#   make aws-source-upload AWS_PROFILE=other AWS_ACCOUNT=123456789012 \
#     S3_BUCKET=agentlens-build-123456789012
# 自己的账号 ID 可以这样看：
#   aws --profile <profile> sts get-caller-identity --query Account --output text
#
# 换区域同理，且桶名会自动跟着区域走（S3 桶是区域性的，CodeBuild 的 S3 源必须同区）：
#   make aws-source-upload AWS_REGION=us-east-2   # -> agentlens-build-use2-<账号>
#   make aws-build-linux   AWS_REGION=us-east-2   # -> us-east-2 里的 agentlens-linux
# CodeBuild project 名是区域内唯一的，所以两个区可以并存同名项目，互不影响。
# ---------------------------------------------------------------------------
# ★★ 这里必须用 := 而不是 ?=（实测踩过）★★
# make 会把**环境变量**当成「已定义」，而本机环境导出了 AWS_REGION=cn-northwest-1，
# 于是 `AWS_REGION ?= us-west-2` 完全不生效，$(AWS) 静默变成
# `aws --region cn-northwest-1 --profile us`，上传报
# `InvalidAccessKeyId: The AWS Access Key Id you provided does not exist in our
# records`——看着像凭据失效，实则是打到了中国分区的端点。
# := 让 makefile 赋值压过环境变量，而命令行 `make aws-status AWS_REGION=us-east-1`
# 依然优先（命令行变量高于 makefile 赋值），换账号的能力一点没丢。
AWS_PROFILE := us
AWS_REGION := us-west-2
# ---------------------------------------------------------------------------
# ★★ 账号 ID 必须「惰性解析且只解析一次」，不能用 := ★★
# 本仓库是公开仓库，所以账号 ID 不写进文件，改为运行期从 STS 现取。但这里有一个
# 必须避开的陷阱：`AWS_ACCOUNT := $(shell aws sts get-caller-identity ...)` 是
# **解析期**求值，于是 `make test` / `make lint` / 甚至 `make help` 都会打一次
# STS——在没有 AWS 凭据的机器上（CI、别人的 clone）要么多等一个网络超时，要么
# 直接失败，而这些目标跟 AWS 毫无关系。
#
# 下面用的是 GNU make 的惰性 memo 惯用法：
#   VAR = $(eval VAR := <真正的求值>)$(VAR)
# 第一次展开 $(AWS_ACCOUNT) 时，$(eval) 把 AWS_ACCOUNT 就地重定义成一个简单变量
# （值已是 STS 结果），$(eval) 本身展开为空，随后的 $(AWS_ACCOUNT) 读到的就是那个
# 简单变量。第二次起不再有 shell 调用。只有真的展开到它的 aws-* recipe 才付这个
# 代价；S3_BUCKET / AWS_ACCOUNT 在本文件里也只被 recipe 用到（aws-source-upload、
# aws-status），别的目标碰不到。
#
# 覆盖顺序：命令行 `make aws-status AWS_ACCOUNT=123456789012` 最高（命令行变量压过
# makefile 赋值，且此时下面这条赋值整体被忽略，连 STS 都不会打）；其次是
# AGENTLENS_AWS_ACCOUNT 环境变量；最后才是 STS。
#
# ★ 环境变量刻意不叫 AWS_ACCOUNT ★
# 理由和上面 AWS_REGION 那段同源：makefile 里的赋值会压过同名环境变量，所以
# `AWS_ACCOUNT=... make aws-status` 会被静默忽略。换个名字就不存在这个歧义。
_AWS_ACCOUNT_FROM_STS = $(strip $(shell $(AWS) sts get-caller-identity \
	--query Account --output text 2>/dev/null))
# $(or) 从左到右求值并在第一个非空处停止（后面的参数不再展开），所以 $(error) 只在
# 两条来源都空时才触发——空账号会拼出 s3://agentlens-build-/ 这种既不存在又难排查
# 的桶名，宁可在这里红。
_AWS_ACCOUNT_RESOLVE = $(or $(AGENTLENS_AWS_ACCOUNT),$(_AWS_ACCOUNT_FROM_STS),$(error \
	无法确定 AWS 账号 ID：aws --region $(AWS_REGION) --profile $(AWS_PROFILE) sts get-caller-identity 没有返回。\
	请确认凭据可用，或显式指定：make $(MAKECMDGOALS) AWS_ACCOUNT=<12 位账号>))
AWS_ACCOUNT = $(eval AWS_ACCOUNT := $(_AWS_ACCOUNT_RESOLVE))$(AWS_ACCOUNT)
# 桶名按区域派生（S3 桶是区域性的，CodeBuild 的 S3 源必须同区）。
# us-west-2 的桶名不带区域短标，那是最早的历史约定，改名会作废已归档的构建证据。
# us-east-2 起统一带短标。未列出的区域走 agentlens-build-<无连字符区域>-<账号>，
# 所以换区不用改 recipe：make aws-build-linux AWS_REGION=us-east-2 即可。
# 这三条同样必须是 =（递归展开）而不是 :=：任何一条用 := 都会在解析期展开
# $(AWS_ACCOUNT)，把上面那套惰性全部作废。
S3_BUCKET_us-west-2 = agentlens-build-$(AWS_ACCOUNT)
S3_BUCKET_us-east-2 = agentlens-build-use2-$(AWS_ACCOUNT)
S3_BUCKET = $(or $(S3_BUCKET_$(AWS_REGION)),agentlens-build-$(subst -,,$(AWS_REGION))-$(AWS_ACCOUNT))
AWS_SRC_KEY := source/agentlens-src.zip
AWS_SRC_ZIP := target/aws/agentlens-src.zip
AWS_PROJECT_PREFIX := agentlens
AWS_PLATFORMS := linux windows macos
# 再补一道保险：把纠正后的区域回灌进环境，这样即使某条 recipe 里漏写 --region，
# 也不会落回 cn-northwest-1。
export AWS_DEFAULT_REGION := $(AWS_REGION)
export AWS_REGION := $(AWS_REGION)
# 唯一的 aws 入口：--region / --profile 固化在这里，杜绝落到错误分区。
AWS := aws --region $(AWS_REGION) --profile $(AWS_PROFILE)
# aws-logs 用：make aws-logs BUILD_ID=agentlens-linux:xxxx（FOLLOW=1 持续跟随）
BUILD_ID ?=
FOLLOW ?=
AWS_LOG_SINCE ?= 4h

.DEFAULT_GOAL := help
.PHONY: help fmt fmt-check lint test test-unit test-e2e test-e2e-real build dev clean \
	coverage coverage-gate hooks \
	dist dist-all dist-reset dist-version dist-clean dist-collector-x86_64 dist-collector-aarch64 \
	dist-askpass dist-stage dist-bundle dist-collect dist-verify \
	aws-source-upload aws-build-linux aws-build-windows aws-build-macos aws-status aws-logs

help: ## 显示可用目标
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-22s\033[0m %s\n", $$1, $$2}'

fmt: ## 格式化 Rust + 前端 TS/TSX + YAML/JSON/Markdown
	cargo fmt --all
	$(NPM) run format
	$(NPM) run lint -- --fix || true
	@if [[ -n '$(OXFMT)' ]]; then \
	  oxfmt --write $(OXFMT_ARGS) $(OXFMT_GLOBS); \
	else \
	  echo '[fmt] $(OXFMT_HINT)'; \
	fi

fmt-check: ## 校验格式（CI 用，不修改文件）
	cargo fmt --all -- --check
	@if [[ -n '$(OXFMT)' ]]; then \
	  oxfmt --check $(OXFMT_ARGS) $(OXFMT_GLOBS); \
	else \
	  echo '[fmt-check] $(OXFMT_HINT)'; \
	fi

lint: fmt-check ## Rust clippy 零告警 + 前端格式 / lint / 类型检查 / 中文字典门禁
	cargo clippy --workspace --all-targets -- -D warnings
	$(NPM) run format:check
	$(NPM) run lint
	$(NPM) run typecheck
	node scripts/check-i18n.mjs

test: ## 运行 Rust 测试
	cargo test --workspace

test-unit: ## 单测层：Rust 单元/集成测试 + 前端 Vitest（不含 e2e）
	cargo test --workspace
	$(NPM) run test:unit

coverage: ## Rust 覆盖率报告 → artifacts/coverage/{summary.txt,lcov.info}
	@mkdir -p $(COVERAGE_DIR)
	@# 先删旧产物：中途失败时留下的是**缺文件**（下游立刻报错），而不是上一次的
	@# 报告被当成本次结果读走。cargo-llvm-cov 自身也会清理上一轮的 profraw。
	rm -f $(COVERAGE_LCOV) $(COVERAGE_SUMMARY)
	cargo llvm-cov --workspace --no-report
	@# 同一份 profdata 出两种报告，测试只跑一次。
	cargo llvm-cov report --summary-only | tee $(COVERAGE_SUMMARY)
	cargo llvm-cov report --lcov --output-path $(COVERAGE_LCOV)
	@test -s $(COVERAGE_LCOV) || { echo '$(COVERAGE_LCOV) 缺失或为空'; exit 1; }
	@echo '[coverage] $(COVERAGE_SUMMARY) 与 $(COVERAGE_LCOV) 已生成'

coverage-gate: ## 行覆盖率低于门槛即失败（可覆盖：make coverage-gate COVERAGE_MIN=99.9）
	@# 先校验门槛再跑覆盖率：门槛写错时 1 秒内失败，而不是白跑几分钟插桩测试。
	@# 也因此这里用递归 make 而非 prerequisite —— prerequisite 一定先于本 recipe 执行。
	@[[ '$(COVERAGE_MIN)' =~ ^[0-9]+([.][0-9]+)?$$ ]] || { \
	  echo "coverage gate 配置错误：COVERAGE_MIN 必须是非负数字，当前为 '$(COVERAGE_MIN)'"; exit 1; }
	@$(MAKE) --no-print-directory coverage
	@# 直接读 lcov 的 LF/LH 汇总：与 Codecov 消费的是同一份数据，两侧数字同源。
	@awk -F: -v min='$(COVERAGE_MIN)' ' \
	  /^LF:/ { found += $$2 } \
	  /^LH:/ { hit += $$2 } \
	  END { \
	    if (found == 0) { print "coverage gate 失败：lcov 中未找到任何可统计行（LF 合计为 0）"; exit 1 } \
	    pct = hit * 100 / found; \
	    if (pct + 0 < min + 0) { \
	      printf "coverage gate 失败：行覆盖率 %.2f%% < 门槛 %s%%（%d/%d 行）\n", pct, min, hit, found; exit 1 \
	    } \
	    printf "coverage gate 通过：行覆盖率 %.2f%% >= 门槛 %s%%（%d/%d 行）\n", pct, min, hit, found \
	  }' $(COVERAGE_LCOV)

hooks: ## 安装 git 钩子（pre-commit → make fmt，pre-push → make test）
	@if [[ '$(HOOKS_MODE)' == 'auto' ]] && command -v pre-commit >/dev/null 2>&1; then \
	  pre-commit install --hook-type pre-commit --hook-type pre-push; \
	  echo '[hooks] 已通过 pre-commit 框架安装（配置见 .pre-commit-config.yaml）'; \
	else \
	  if [[ '$(HOOKS_MODE)' == 'auto' ]]; then \
	    echo '[hooks] 未安装 pre-commit 框架，退化为写入 .git/hooks/ 纯脚本'; \
	  else \
	    echo '[hooks] HOOKS_MODE=$(HOOKS_MODE)，强制写入 .git/hooks/ 纯脚本'; \
	  fi; \
	  dir=$$(git rev-parse --git-path hooks); \
	  mkdir -p "$$dir"; \
	  root=$$(git rev-parse --show-toplevel); \
	  for spec in 'pre-commit:fmt' 'pre-push:test'; do \
	    hook=$${spec%%:*}; target=$${spec##*:}; \
	    printf '%s\n' '#!/bin/sh' \
	      '# 由 `make hooks` 生成（HOOKS_MODE=plain 或缺少 pre-commit 框架时的等价实现）。' \
	      "cd \"$$root\" || exit 1" \
	      "exec make $$target" > "$$dir/$$hook"; \
	    chmod +x "$$dir/$$hook"; \
	    echo "[hooks] 已写入 $$dir/$$hook → make $$target"; \
	  done; \
	fi

test-e2e: ## 前端组件级 QA（Playwright + mock IPC，截图落 artifacts/qa/）
	$(NPM) run test:e2e

test-e2e-real: ## 真实端到端 QA（tauri-driver + WebdriverIO，需先 cargo build 与 vite dev）
	$(NPM) run test:e2e:real

build: ## 构建 Rust workspace 与前端产物
	cargo build --workspace
	$(NPM) run build

dev: ## 启动 Tauri 开发模式
	cargo tauri dev

# ---------------------------------------------------------------------------
# 打包流水线
#
# 产物（$(DIST_DIR)/）：
#   AgentLens_<version>_amd64.deb            桌面安装包（内含 askpass 与双架构 collector）
#   agentlens-collector-x86_64-...-musl      独立 collector（供手工分发到远端）
#   agentlens-collector-aarch64-...-musl     同上（工具链缺失时缺席并在清单里注明）
#   sha256sums.txt                            覆盖上述全部文件，`sha256sum -c` 可校验
#
# 刻意不产 AppImage / rpm / msi：计划只交付 Linux deb + Windows NSIS。
# ---------------------------------------------------------------------------

dist: ## 打包 Linux：deb + musl collector + sha256sums.txt（aarch64 缺工具链只告警）
	@$(MAKE) --no-print-directory dist-reset
	@$(MAKE) --no-print-directory dist-collect
	@$(MAKE) --no-print-directory dist-verify

dist-all: ## 同 dist，但强制要求 aarch64 collector 存在（发布期与 CI 用）
	@$(MAKE) --no-print-directory dist-reset
	@$(MAKE) --no-print-directory DIST_REQUIRE_AARCH64=1 dist-collect
	@$(MAKE) --no-print-directory dist-verify

# 构建开始前就清空产物目录：中途被打断时留下的是**空目录**（dist-verify 会直接失败），
# 而不是上一次的完整产物——后者虽然自洽，却会被误当成本次构建的输出发出去。
dist-reset:
	rm -rf $(DIST_DIR)

dist-version: ## 打印从 workspace 解析出的版本号（单一事实源自检）
	@printf 'workspace version = %s\nhost triple      = %s\n' '$(VERSION)' '$(HOST_TRIPLE)'
	@test -n '$(VERSION)' || { echo '无法从 Cargo.toml 解析 [workspace.package].version'; exit 1; }

dist-clean: ## 清理打包产物（不动 cargo 缓存）
	rm -rf $(DIST_DIR) target/dist

dist-collector-x86_64: dist-version ## 构建 x86_64 静态 musl collector
	rm -f $(COLLECTOR_X86_BUILT)
	cargo build -p agentlens-collector --target $(MUSL_X86_TARGET) --release
	@test -x $(COLLECTOR_X86_BUILT) || { echo '缺少 $(COLLECTOR_X86_BUILT)'; exit 1; }

dist-collector-aarch64: dist-version ## 构建 aarch64 静态 musl collector（缺交叉编译器时告警跳过）
	@rm -f $(COLLECTOR_ARM_BUILT)
	@cc='$(AARCH64_MUSL_CC)'; ar=''; rustflags=''; \
	if [[ -z "$$cc" && -n '$(ZIG)' ]]; then \
	  mkdir -p $(TOOLCHAIN_DIR); \
	  { \
	    echo '#!/bin/sh'; \
	    echo '# zig 只认三段式 target；cc-rs 额外传的 --target=<rust triple> 会让 zig 报'; \
	    echo '# UnknownOperatingSystem，用 shift 轮转重建参数表剔掉它（保留含空格的参数）。'; \
	    echo 'count=$$#'; \
	    echo 'i=0'; \
	    echo 'while [ "$$i" -lt "$$count" ]; do'; \
	    echo '  arg="$$1"; shift'; \
	    echo '  case "$$arg" in --target=*) ;; *) set -- "$$@" "$$arg" ;; esac'; \
	    echo '  i=$$((i + 1))'; \
	    echo 'done'; \
	    echo 'exec zig cc -target aarch64-linux-musl "$$@"'; \
	  } > $(TOOLCHAIN_DIR)/aarch64-musl-cc; \
	  printf '%s\n%s\n' '#!/bin/sh' 'exec zig ar "$$@"' > $(TOOLCHAIN_DIR)/aarch64-musl-ar; \
	  chmod +x $(TOOLCHAIN_DIR)/aarch64-musl-cc $(TOOLCHAIN_DIR)/aarch64-musl-ar; \
	  cc="$$PWD/$(TOOLCHAIN_DIR)/aarch64-musl-cc"; \
	  ar="$$PWD/$(TOOLCHAIN_DIR)/aarch64-musl-ar"; \
	  export ZIG_GLOBAL_CACHE_DIR="$$PWD/target/dist/zig-cache"; \
	  echo '[dist] aarch64 musl C 编译器：zig cc -target aarch64-linux-musl'; \
	  rustflags='-C link-self-contained=no'; \
	fi; \
	if [[ -z "$$cc" ]]; then \
	  echo '################################################################'; \
	  echo '# 警告：未找到 aarch64 musl C 交叉编译器（aarch64-linux-musl-gcc 或 zig）。'; \
	  echo '# rusqlite 的 bundled feature 需要它编译 sqlite3.c，故 aarch64 collector 缺席。'; \
	  echo '# 本次打包只含 x86_64 collector，sha256sums.txt 会显式注明缺席，绝不伪造产物。'; \
	  echo '# 解除办法：安装 zig 或 aarch64-linux-musl-gcc，或设 AARCH64_MUSL_CC=<路径>，'; \
	  echo '# 或在真实 aarch64 主机 / CI 原生构建。发布请用 make dist-all（缺席即失败）。'; \
	  echo '################################################################'; \
	  exit 0; \
	fi; \
	env $${ar:+AR_$(subst -,_,$(MUSL_ARM_TARGET))=$$ar} \
	    CC_$(subst -,_,$(MUSL_ARM_TARGET))="$$cc" \
	    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="$$cc" \
	    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="$$rustflags" \
	    cargo build -p agentlens-collector --target $(MUSL_ARM_TARGET) --release
	@if [[ -x $(COLLECTOR_ARM_BUILT) ]]; then \
	  echo "[dist] aarch64 collector: $$(file -b $(COLLECTOR_ARM_BUILT))"; \
	fi

dist-askpass: dist-version ## 构建 SSH_ASKPASS 助手（随桌面包分发）
	cargo build -p agentlens-askpass --release
	@test -x $(ASKPASS_BUILT) || { echo '缺少 $(ASKPASS_BUILT)'; exit 1; }

dist-stage: dist-collector-x86_64 dist-collector-aarch64 dist-askpass ## 按 tauri externalBin 命名约定归置 sidecar
	rm -rf $(STAGE_DIR)
	mkdir -p $(STAGE_DIR)
	cp $(COLLECTOR_X86_BUILT) $(STAGE_DIR)/$(COLLECTOR_X86)-$(HOST_TRIPLE)
	cp $(ASKPASS_BUILT) $(STAGE_DIR)/$(ASKPASS_BIN)-$(HOST_TRIPLE)
	@if [[ -x $(COLLECTOR_ARM_BUILT) ]]; then \
	  cp $(COLLECTOR_ARM_BUILT) $(STAGE_DIR)/$(COLLECTOR_ARM)-$(HOST_TRIPLE); \
	else \
	  echo '[dist] 跳过 aarch64 sidecar（上一步已告警）'; \
	fi
	chmod 0755 $(STAGE_DIR)/*
	@# 随包分发的 collector 校验清单：安装后可核对 sidecar 未被篡改。
	@# 名字必须写成**安装后**的名字（bundler 会去掉 `-<host triple>` 后缀），
	@# 否则用户在 /usr/bin 里 `sha256sum -c` 会全部 No such file。
	@cd $(STAGE_DIR) && sha256sum agentlens-collector-* \
	  | sed 's/-$(HOST_TRIPLE)$$//' > collectors.sha256 && cat collectors.sha256
	@# externalBin 里声明的文件必须齐全，否则 tauri 会以 ResourcePathNotFound 失败；
	@# aarch64 缺席时改用只含现存产物的降级配置，绝不让 bundler 静默少打一个文件。
	@if [[ -x $(STAGE_DIR)/$(COLLECTOR_ARM)-$(HOST_TRIPLE) ]]; then \
	  cp $(BUNDLE_CONFIG) target/dist/bundle-config.json; \
	else \
	  python3 -c 'import json, sys; c = json.load(open(sys.argv[1])); \
b = c["bundle"]; \
b["externalBin"] = [p for p in b["externalBin"] if "aarch64" not in p]; \
json.dump(c, open(sys.argv[2], "w"), indent=2, ensure_ascii=False)' \
	    $(BUNDLE_CONFIG) target/dist/bundle-config.json; \
	  echo '[dist] 使用降级 bundle 配置（无 aarch64 sidecar）'; \
	fi

dist-bundle: dist-stage ## 构建前端 + 桌面 deb（内含 askpass 与 collector sidecar）
	rm -rf $(DEB_DIR)
	cargo tauri build --bundles deb --config target/dist/bundle-config.json

dist-collect: dist-bundle ## 归集全部发布产物并生成 sha256sums.txt
	@rm -rf $(DIST_DIR)
	@mkdir -p $(DIST_DIR)
	@deb=$$(ls -1 $(DEB_DIR)/*.deb 2>/dev/null | head -1); \
	test -n "$$deb" || { echo '未找到 deb 产物（$(DEB_DIR)/*.deb）'; exit 1; }; \
	case "$$deb" in *_$(VERSION)_*) ;; \
	  *) echo "deb 文件名 $$deb 未包含 workspace 版本 $(VERSION)"; exit 1 ;; \
	esac; \
	cp "$$deb" $(DIST_DIR)/
	@cp $(COLLECTOR_X86_BUILT) $(DIST_DIR)/$(COLLECTOR_X86)
	@if [[ -x $(COLLECTOR_ARM_BUILT) ]]; then cp $(COLLECTOR_ARM_BUILT) $(DIST_DIR)/$(COLLECTOR_ARM); fi
	@if [[ '$(DIST_REQUIRE_AARCH64)' != '0' && ! -f $(DIST_DIR)/$(COLLECTOR_ARM) ]]; then \
	  echo '错误：DIST_REQUIRE_AARCH64 已开启，但缺少 $(COLLECTOR_ARM)'; exit 1; \
	fi
	@arm_state='缺席（无 aarch64 musl C 交叉编译器，见构建日志告警）'; \
	if [[ -f $(DIST_DIR)/$(COLLECTOR_ARM) ]]; then arm_state='存在'; fi; \
	test -n "$$(ls -1 $(DIST_DIR))" || { echo '产物目录为空，拒绝生成空清单'; exit 1; }; \
	cd $(DIST_DIR) && { \
	  printf '# AgentLens %s 发布产物校验清单（sha256sum -c 可直接校验）\n' '$(VERSION)'; \
	  printf '# collector 目标：x86_64-unknown-linux-musl = 存在；aarch64-unknown-linux-musl = %s\n' "$$arm_state"; \
	  sha256sum $$(ls -1 | grep -v '^sha256sums.txt$$' | sort); \
	} > sha256sums.txt
	@echo '[dist] 产物清单：'
	@ls -l $(DIST_DIR)
	@cat $(DIST_DIR)/sha256sums.txt

dist-verify: ## 校验 sha256sums.txt 与目录内容一致（缺文件 / 哈希不符即失败）
	@test -f $(DIST_DIR)/sha256sums.txt || { echo '缺少 $(DIST_DIR)/sha256sums.txt'; exit 1; }
	@cd $(DIST_DIR) && sha256sum -c --strict sha256sums.txt
	@# 反向核对：目录里不能存在清单未收录的产物，否则「校验通过」是假象。
	@cd $(DIST_DIR) && \
	  listed=$$(sed '/^#/d' sha256sums.txt | sed 's/^[0-9a-f]*  //' | sort) && \
	  present=$$(ls -1 | grep -v '^sha256sums.txt$$' | sort) && \
	  if [[ "$$listed" != "$$present" ]]; then \
	    echo '产物目录与清单不一致：'; diff <(echo "$$listed") <(echo "$$present") || true; exit 1; \
	  fi
	@echo '[dist] sha256sums.txt 校验通过，且与目录内容完全一致'

clean: ## 清理构建产物
	cargo clean
	rm -rf $(FRONTEND)/dist $(DIST_DIR)

# ---------------------------------------------------------------------------
# AWS CodeBuild 目标
#
# 分工：本文件与 .aws/buildspec/*.yml 是**共享面**，三个平台负责人各自建自己的
# CodeBuild project 并只迭代自己那份 buildspec，互不改同一个文件。
# create-project 的命令形状见 .aws/README.md。
#
# buildspec 里刻意不含任何账号信息（account id / bucket / role ARN），
# 那些只出现在这里和 create-project 调用中。
# ---------------------------------------------------------------------------

aws-source-upload: ## 打包工作树并上传到 S3（CodeBuild 的 S3 源；本仓库尚无 git remote）
	@# 排除清单要和 .gitignore 的重物保持一致：target/ 是 cargo 产物（GB 级），
	@# node_modules/ 由 buildspec 内的 npm ci 从 lockfile 重建，artifacts/ 与
	@# frontend/dist/ 是构建输出，.omo/ 是编排状态，与远端构建无关。
	@#
	@# ★★ .git/ 必须进 zip——这不是可选项（实测踩过）★★
	@# buildspec 里那条 ts-rs 生成物漂移门禁靠
	@#   git status --porcelain -uall -- frontend/src/generated/
	@# 判断「重新导出后的字节」与「提交进仓库的字节」是否一致。没有 .git/ 就没有
	@# 索引可比，那条命令恒为空，整个门禁**静默退化成「导出本身没崩」**——它永远
	@# 绿，包括在 Rust DTO 与提交的 .ts 已经不一致的时候。这不是「可接受的降级」，
	@# 是一个装饰性门禁。所以 .git/ 现在是**源包的必需内容**，下面还有一条正向
	@# 断言守着它，谁再把 .git/ 排除掉，打包这一步就直接失败。
	@# 代价：zip 从 ~900 KB 涨到 ~8 MB（.git 全是松散对象），S3 上传与
	@# DOWNLOAD_SOURCE 各多几秒；换来门禁真能红。
	@echo '[aws] region=$(AWS_REGION) profile=$(AWS_PROFILE) bucket=$(S3_BUCKET)'
	@mkdir -p $(dir $(AWS_SRC_ZIP))
	@rm -f $(AWS_SRC_ZIP)
	@start=$$(date +%s); \
	zip -r -q -X $(AWS_SRC_ZIP) . \
	  -x 'target/*' 'frontend/node_modules/*' 'frontend/dist/*' 'artifacts/*' '.omo/*'; \
	elapsed=$$(( $$(date +%s) - start )); \
	count=$$(zipinfo -1 $(AWS_SRC_ZIP) | wc -l); \
	size=$$(du -h $(AWS_SRC_ZIP) | cut -f1); \
	echo "[aws] 打包完成：$(AWS_SRC_ZIP)  大小 $$size  条目 $$count  耗时 $${elapsed}s"
	@# 反向自检：排除目录若泄漏进 zip，宁可这里失败，也不要让远端拉一个 GB 级源包。
	@if zipinfo -1 $(AWS_SRC_ZIP) | grep -E '^(target/|artifacts/|\.omo/)|node_modules/'; then \
	  echo '[aws] 排除失败：上列条目本应被排除'; exit 1; \
	fi
	@# 正向自检：git 元数据必须在包里，否则漂移门禁在远端是装饰品。三样都要：
	@# HEAD + refs（解析当前提交）、index（worktree↔索引比对的基线）、objects
	@# （失败路径上 git diff 要读 blob 才能打印真实差异）。
	@#
	@# ★ 这里刻意不用 `zipinfo | grep -q`（实测踩过）★
	@# .SHELLFLAGS 带 -o pipefail，而 `grep -q` 命中即退出会让上游 zipinfo 吃到
	@# SIGPIPE，整条管道退 141，于是「找到了」被判成「没找到」——一条本该守门的
	@# 断言反过来把好包拦下来。清单只取一次，之后纯 bash 匹配，不再有管道。
	@list=$$(zipinfo -1 $(AWS_SRC_ZIP)); \
	for entry in '.git/HEAD' '.git/index'; do \
	  case $$'\n'"$$list"$$'\n' in \
	    *$$'\n'"$$entry"$$'\n'*) ;; \
	    *) echo "[aws] 源包缺 $$entry：漂移门禁会退化成「导出成功」，拒绝上传"; exit 1;; \
	  esac; \
	done; \
	objs=0; \
	while IFS= read -r line; do \
	  case "$$line" in .git/objects/*) objs=$$((objs + 1));; esac; \
	done <<< "$$list"; \
	test "$$objs" -gt 0 || { \
	  echo '[aws] 源包里 .git/objects/ 为空：git diff 无法读 blob，拒绝上传'; exit 1; }; \
	echo "[aws] git 元数据自检通过：.git/HEAD + .git/index + $$objs 个 .git/objects/ 条目"
	$(AWS) s3 cp $(AWS_SRC_ZIP) s3://$(S3_BUCKET)/$(AWS_SRC_KEY)
	@# 不信 CLI 的退出码：回读 S3 上的对象大小与时间戳，确认上传真的落地了。
	@echo '[aws] S3 实际对象：'
	@$(AWS) s3api head-object --bucket $(S3_BUCKET) --key $(AWS_SRC_KEY) \
	  --query '{ContentLength:ContentLength,LastModified:LastModified,VersionId:VersionId}' --output table

aws-build-linux: ## 触发 agentlens-linux 构建（全量门禁 + make dist-all）
	@$(MAKE) --no-print-directory aws-start-build AWS_PLATFORM=linux

aws-build-windows: ## 触发 agentlens-windows 构建（Rust 门禁 + NSIS 安装包）
	@$(MAKE) --no-print-directory aws-start-build AWS_PLATFORM=windows

aws-build-macos: ## 触发 agentlens-macos 构建（Rust 门禁 + dmg；fleet 容量 1，需串行）
	@$(MAKE) --no-print-directory aws-start-build AWS_PLATFORM=macos

# 内部目标：三个 aws-build-* 的共同实现，避免三份重复 recipe。
.PHONY: aws-start-build
aws-start-build:
	@test -n '$(AWS_PLATFORM)' || { echo '需要 AWS_PLATFORM=linux|windows|macos'; exit 1; }
	@project='$(AWS_PROJECT_PREFIX)-$(AWS_PLATFORM)'; \
	echo "[aws] start-build $$project @ $(AWS_REGION)"; \
	id=$$($(AWS) codebuild start-build --project-name "$$project" \
	  --query 'build.id' --output text); \
	echo "[aws] build id: $$id"; \
	echo "[aws] 查看日志：make aws-logs BUILD_ID=$$id"

aws-status: ## 显示三个平台各自最近一次构建的状态（可加 AWS_REGION=us-east-2 切区）
	@echo '[aws] region=$(AWS_REGION) bucket=$(S3_BUCKET)'
	@for plat in $(AWS_PLATFORMS); do \
	  project="$(AWS_PROJECT_PREFIX)-$$plat"; \
	  id=$$($(AWS) codebuild list-builds-for-project --project-name "$$project" \
	    --sort-order DESCENDING --query 'ids[0]' --output text 2>/dev/null || echo 'ERR'); \
	  if [[ "$$id" == 'ERR' ]]; then \
	    printf '  %-22s %s\n' "$$project" '项目不存在（由该平台负责人创建）'; \
	  elif [[ "$$id" == 'None' || -z "$$id" ]]; then \
	    printf '  %-22s %s\n' "$$project" '项目已存在，尚无构建记录'; \
	  else \
	    $(AWS) codebuild batch-get-builds --ids "$$id" \
	      --query 'builds[0].{Status:buildStatus,Phase:currentPhase,Start:startTime,Id:id}' \
	      --output text | sed "s|^|  |"; \
	  fi; \
	done

aws-logs: ## 拉取指定构建的 CloudWatch 日志（make aws-logs BUILD_ID=... [FOLLOW=1]）
	@test -n '$(BUILD_ID)' || { \
	  echo '需要 BUILD_ID=<project>:<uuid>（aws-build-* 会打印）'; \
	  echo '可用 make aws-status 找最近一次构建的 id'; exit 1; }
	@# 日志组/流名从构建元数据里取，而不是拼 `/aws/codebuild/<project>`：
	@# 后者在项目自定义了 logsConfig 之后就会指向错误的组。
	@read -r group stream < <($(AWS) codebuild batch-get-builds --ids '$(BUILD_ID)' \
	  --query 'builds[0].logs.[groupName,streamName]' --output text); \
	if [[ -z "$$group" || "$$group" == 'None' ]]; then \
	  echo '未拿到日志组：BUILD_ID 是否正确？构建是否刚开始（日志尚未创建）？'; exit 1; \
	fi; \
	echo "[aws] log group=$$group stream=$$stream"; \
	$(AWS) logs tail "$$group" --log-stream-names "$$stream" \
	  --since $(AWS_LOG_SINCE) $(if $(FOLLOW),--follow,)
