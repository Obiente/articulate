export function updateNotices(state) {
  if (!state || state.preview) return [];
  const notices = [];
  const app = state.updater || {};
  const companion = state.discord?.companion || {};

  if (app.state === "available") {
    notices.push({
      id: `app-available-${app.version || "unknown"}`,
      title: `Articulate ${app.version || "update"} is available`,
      message: "Download it in Settings when you are ready.",
      section: "updates",
      color: "mint",
    });
  } else if (app.state === "ready") {
    notices.push({
      id: `app-ready-${app.version || "unknown"}`,
      title: `Articulate ${app.version || "update"} is ready to install`,
      message: "Finish any recording before installing the update.",
      section: "updates",
      color: "mint",
    });
  } else if (app.state === "error") {
    notices.push({
      id: "app-update-error",
      title: "App update needs attention",
      message: app.error || "Check for updates again in Settings.",
      section: "updates",
      color: "red",
    });
  }

  if (companion.error) {
    notices.push({
      id: "companion-update-error",
      title: "Vencord companion needs attention",
      message: companion.error,
      section: "discord",
      color: "red",
    });
  } else if (companion.runtime_status === "restart_required") {
    notices.push({
      id: "companion-restart-required",
      title: "Restart Discord to finish updating",
      message:
        "The Vencord companion is ready. Restarting Discord will disconnect your current call.",
      section: "discord",
      color: "yellow",
    });
  } else if (
    !companion.busy &&
    (companion.runtime_status === "update_required" ||
      companion.plugin_status === "update_available")
  ) {
    notices.push({
      id: "companion-update-available",
      title: "Vencord companion update available",
      message: "Update the companion in Settings for the latest audio support.",
      section: "discord",
      color: "mint",
    });
  }
  return notices;
}

export function createUpdateNoticeTracker() {
  let previous = new Set();
  return (notices) => {
    const current = new Set(notices.map((notice) => notice.id));
    const newlyActive = notices.filter((notice) => !previous.has(notice.id));
    previous = current;
    return newlyActive;
  };
}
