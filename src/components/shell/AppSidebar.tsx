import {
  ActivityIcon,
  AudioWaveformIcon,
  CalendarClockIcon,
  ClipboardCopyIcon,
  FilesIcon,
  FileTextIcon,
  Settings2Icon,
  WebhookIcon,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import appIcon from "@/assets/wakenote-app.png";
import { PRIMARY_NAV, type PrimaryRoute } from "@/lib/navigation";
import { Button } from "@/components/ui/button";
import { StatusBadge } from "@/components/ui/status-badge";
import { AppUpdateControl } from "./AppUpdateControl";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

const NAV_ICONS: Record<(typeof PRIMARY_NAV)[number]["id"], LucideIcon> = {
  capture: AudioWaveformIcon,
  live: ClipboardCopyIcon,
  meetings: CalendarClockIcon,
  transcripts: FilesIcon,
  reports: FileTextIcon,
  activity: ActivityIcon,
  webhooks: WebhookIcon,
};

function attentionCountLabel(count: number): string {
  return count > 99 ? "99+" : String(count);
}

export function AppSidebar({
  activeRoute,
  queueAttentionCount,
  queueAttentionTone,
  onNavigate,
}: {
  activeRoute: PrimaryRoute;
  queueAttentionCount: number;
  queueAttentionTone: "warning" | "danger";
  onNavigate: (route: PrimaryRoute) => void;
}) {
  const settingsActive = activeRoute === "settings" || activeRoute === "costs";
  return (
    <TooltipProvider delayDuration={300}>
      <aside className="app-sidebar">
        <div className="app-sidebar__brand">
          <span className="app-sidebar__mark" aria-hidden="true">
            <img src={appIcon} alt="" />
          </span>
          <span className="app-sidebar__brand-copy">
            <strong>WakeNote</strong>
            <small>Recorder workspace</small>
          </span>
        </div>

        <nav className="app-sidebar__nav" aria-label="Primary navigation">
          {PRIMARY_NAV.map((item) => {
            const Icon = NAV_ICONS[item.id];
            const active = activeRoute === item.id;
            return (
              <Tooltip key={item.id}>
                <TooltipTrigger asChild>
                  <Button
                    type="button"
                    size="sm"
                    variant={active ? "secondary" : "ghost"}
                    className="app-sidebar__nav-item"
                    data-route={item.id}
                    data-active={active}
                    aria-label={item.label}
                    aria-current={active ? "page" : undefined}
                    onClick={() => onNavigate(item.id)}
                  >
                    <Icon data-icon="inline-start" />
                    <span className="app-sidebar__nav-label">{item.label}</span>
                    {item.id === "activity" && queueAttentionCount > 0 ? (
                      <StatusBadge tone={queueAttentionTone}>
                        {attentionCountLabel(queueAttentionCount)}
                      </StatusBadge>
                    ) : null}
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="right" sideOffset={8}>
                  {item.label}
                </TooltipContent>
              </Tooltip>
            );
          })}
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                type="button"
                size="sm"
                variant={settingsActive ? "secondary" : "ghost"}
                className="app-sidebar__nav-item"
                data-route="settings"
                data-active={settingsActive}
                aria-label="Settings"
                aria-current={settingsActive ? "page" : undefined}
                onClick={() => onNavigate("settings")}
              >
                <Settings2Icon data-icon="inline-start" />
                <span className="app-sidebar__nav-label">Settings</span>
              </Button>
            </TooltipTrigger>
            <TooltipContent side="right" sideOffset={8}>
              Settings
            </TooltipContent>
          </Tooltip>
        </nav>

        <div className="app-sidebar__utility">
          <AppUpdateControl />
        </div>
      </aside>
    </TooltipProvider>
  );
}
