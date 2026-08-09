/**
 * Shell navigation: five tabs, one per view.
 *
 * Owner: W8 prep (shell/infrastructure). Labels come from the `zh.nav` block, which the
 * shell owns; view workers add strings under their own `zh.<view>` section instead.
 */
import { Button } from '@/components/ui/button'
import { VIEW_KEYS, type ViewKey } from '@/app/views'
import { zh } from '@/i18n/zh'

export function AppNav({
  active,
  onSelect,
}: {
  active: ViewKey
  onSelect: (view: ViewKey) => void
}) {
  return (
    <nav role="tablist" aria-label={zh.appName} className="flex flex-wrap gap-1">
      {VIEW_KEYS.map((view) => (
        <Button
          key={view}
          role="tab"
          aria-selected={view === active}
          data-testid={`nav-${view}`}
          variant={view === active ? 'default' : 'ghost'}
          size="sm"
          onClick={() => onSelect(view)}
        >
          {zh.nav[view]}
        </Button>
      ))}
    </nav>
  )
}
