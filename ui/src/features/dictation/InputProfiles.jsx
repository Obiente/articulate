import { useState } from "react";
import {
  Button,
  Group,
  Paper,
  Select,
  Stack,
  Text,
  TextInput,
} from "@mantine/core";
import { recentApps, replaceProfile, styleOptions } from "./profile-options.js";
const inherit = { value: "", label: "Use default" };
const booleanOptions = [
  inherit,
  { value: "true", label: "On" },
  { value: "false", label: "Off" },
];
export function InputProfiles({ controls }) {
  const { settings, state, patch, busy, capturing } = controls;
  const [draft, setDraft] = useState(null);
  const [styleApp, setStyleApp] = useState("");
  const [style, setStyle] = useState("Clean");
  const profiles = settings.input_profiles || [];
  const update = (key, value) => setDraft({ ...draft, [key]: value });
  return (
    <Paper withBorder radius="lg" p="lg" mt="md">
      <Stack>
        <Text fw={600}>App profiles</Text>
        <Text size="sm" c="dimmed">
          Overrides apply when you start dictating in that app. Other values use
          your defaults. Discord, Slack and Teams use Shift+Enter for line
          breaks.
        </Text>
        {profiles.map((profile) => (
          <Group key={profile.app}>
            <Text>{profile.app}</Text>
            <Button variant="subtle" onClick={() => setDraft({ ...profile })}>
              Edit
            </Button>
            <Button
              variant="subtle"
              color="red"
              disabled={busy || capturing}
              onClick={() =>
                patch(
                  "input_profiles",
                  profiles.filter((p) => p.app !== profile.app),
                )
              }
            >
              Remove
            </Button>
          </Group>
        ))}
        <Select
          label="Add from recent apps"
          placeholder="Choose an app"
          data={recentApps(settings, state.insights?.apps || [])}
          value={null}
          onChange={(app) => app && setDraft({ app })}
        />
        <Button variant="default" onClick={() => setDraft({ app: "" })}>
          Add app profile
        </Button>
        {draft && (
          <Stack>
            <TextInput
              label="Executable name"
              placeholder="editor.exe"
              value={draft.app}
              onChange={(e) => update("app", e.currentTarget.value)}
            />
            <Select
              label="Shortcut behavior"
              data={[
                inherit,
                { value: "Hold", label: "Hold" },
                { value: "Toggle", label: "Toggle" },
              ]}
              value={draft.hotkey_mode || ""}
              onChange={(v) => update("hotkey_mode", v || null)}
            />
            {[
              ["insert", "Insert into app"],
              ["live_insert", "Live typing"],
              ["spoken_editing_commands", "Spoken editing"],
            ].map(([key, label]) => (
              <Select
                key={key}
                label={label}
                data={booleanOptions}
                value={draft[key] == null ? "" : String(draft[key])}
                onChange={(v) => update(key, v === "" ? null : v === "true")}
              />
            ))}
            <Select
              label="Line breaks"
              data={[
                inherit,
                { value: "Enter", label: "Enter" },
                { value: "ShiftEnter", label: "Shift+Enter" },
              ]}
              value={draft.line_break || ""}
              onChange={(v) => update("line_break", v || null)}
            />
            <Select
              label="Writing style"
              data={[inherit, ...styleOptions]}
              value={draft.style || ""}
              onChange={(v) => update("style", v || null)}
            />
            <Group>
              <Button
                disabled={busy || capturing || !draft.app.trim()}
                onClick={async () => {
                  if (
                    await patch(
                      "input_profiles",
                      replaceProfile(profiles, draft),
                    )
                  )
                    setDraft(null);
                }}
              >
                Save profile
              </Button>
              <Button variant="default" onClick={() => setDraft(null)}>
                Cancel
              </Button>
            </Group>
          </Stack>
        )}
        <Text fw={600}>Writing style by app</Text>
        {(settings.styles || []).map((rule) => (
          <Group key={rule.app}>
            <Text>
              {rule.app}:{" "}
              {styleOptions.find((s) => s.value === rule.style)?.label}
            </Text>
            <Button
              variant="subtle"
              disabled={busy || capturing}
              onClick={() =>
                patch(
                  "styles",
                  settings.styles.filter((r) => r.app !== rule.app),
                )
              }
            >
              Remove style
            </Button>
          </Group>
        ))}
        <TextInput
          label="Style executable"
          placeholder="editor.exe"
          value={styleApp}
          onChange={(e) => setStyleApp(e.currentTarget.value)}
        />
        <Select
          label="App writing style"
          data={styleOptions}
          value={style}
          onChange={setStyle}
        />
        <Button
          variant="default"
          disabled={busy || capturing || !styleApp.trim()}
          onClick={() =>
            patch(
              "styles",
              replaceProfile(settings.styles || [], { app: styleApp, style }),
            )
          }
        >
          Save writing style
        </Button>
      </Stack>
    </Paper>
  );
}
