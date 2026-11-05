import type { ReactNode } from "react";
import type { PrimaryRoute } from "@/lib/navigation";

export function AppPageRouter({
  route,
  pages,
}: {
  route: PrimaryRoute;
  pages: Record<PrimaryRoute, ReactNode>;
}) {
  switch (route) {
    case "capture":
      return pages.capture;
    case "live":
      return pages.live;
    case "meetings":
      return pages.meetings;
    case "transcripts":
      return pages.transcripts;
    case "reports":
      return pages.reports;
    case "activity":
      return pages.activity;
    case "webhooks":
      return pages.webhooks;
    case "costs":
      return pages.costs;
    case "settings":
      return pages.settings;
  }
}
