/**
 * Top-level view switcher.
 *
 * Owner: W8 prep (shell/infrastructure). Deliberately **no router dependency**: the shell
 * is sibling tabs in one desktop window with no deep links, no nested routes and no
 * browser history to honour, so a typed union in `useState` is the whole requirement.
 * Adding `react-router` would buy nothing and cost a dependency the plan does not want.
 */
export const VIEW_KEYS = [
  'overview',
  'drilldown',
  'detail',
  'hosts',
  'settings',
  'diagnostics',
] as const

export type ViewKey = (typeof VIEW_KEYS)[number]
