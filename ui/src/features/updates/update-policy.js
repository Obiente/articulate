export function shouldCheckUpdates(enabled, state) {
  return enabled === true && ["idle", "latest", "error"].includes(state);
}
