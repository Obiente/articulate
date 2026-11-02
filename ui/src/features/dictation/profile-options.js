export const styleOptions = [
  { value: "Verbatim", label: "No cleanup" },
  { value: "Clean", label: "Clean" },
  { value: "Chat", label: "Casual chat" },
];
export function recentApps(settings, history = []) {
  return [
    ...new Set(
      [
        settings.recent_dictation_app,
        ...history.map((item) => item.app),
      ].filter(Boolean),
    ),
  ];
}
export function replaceProfile(profiles, profile) {
  return [
    ...profiles.filter(
      (item) => item.app.toLowerCase() !== profile.app.toLowerCase(),
    ),
    profile,
  ];
}
