import { useState } from "react";
import {
  ActionIcon,
  Autocomplete,
  Avatar,
  Badge,
  Button,
  Menu,
  Tooltip,
} from "@mantine/core";
import {
  ArrowCounterClockwise,
  ArrowsMerge,
  Check,
  DotsThreeVertical,
  FunnelSimple,
  PencilSimple,
  X,
} from "@phosphor-icons/react";
import { nameSuggestions } from "./speakers.js";

// An inline rename field with known names as quick picks.
function RenameField({ speaker, suggestions, onSave, onCancel, busy }) {
  const [value, setValue] = useState(
    speaker.kind === "voice" && /^Speaker \d$/.test(speaker.label)
      ? ""
      : speaker.label,
  );
  const save = () => {
    const name = value.trim();
    if (name && name !== speaker.label) onSave(name);
    else onCancel();
  };
  return (
    <div className="speaker-rename">
      <Autocomplete
        aria-label={`New name for ${speaker.label}`}
        placeholder={speaker.label}
        size="xs"
        autoFocus
        data={suggestions}
        value={value}
        onChange={setValue}
        onOptionSubmit={(name) => onSave(name)}
        onKeyDown={(event) => {
          if (event.key === "Enter") save();
          if (event.key === "Escape") onCancel();
        }}
        disabled={busy}
        comboboxProps={{ withinPortal: true }}
      />
      <ActionIcon
        size={26}
        variant="light"
        aria-label="Save name"
        onClick={save}
        loading={busy}
      >
        <Check size={14} />
      </ActionIcon>
      <ActionIcon
        size={26}
        variant="subtle"
        color="gray"
        aria-label="Cancel renaming"
        onClick={onCancel}
      >
        <X size={14} />
      </ActionIcon>
    </div>
  );
}

// Everyone in the conversation, with renaming, merging and focus. Changes
// apply to the transcript, notes sources, exports and search.
export function SpeakerPanel({
  session,
  roster,
  speakerTone,
  people = [],
  speakerFilter,
  onFilter,
  act,
  locked,
  renderAvatar,
}) {
  const [renaming, setRenaming] = useState(null),
    [busy, setBusy] = useState(false);
  const editable = session?.kind === "call" && !locked;
  const run = async (command) => {
    setBusy(true);
    try {
      await act(command);
      return true;
    } catch {
      return false;
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="speaker-panel">
      {roster.map((speaker) => {
        const tone = speakerTone(speaker.label);
        const focused = speakerFilter === speaker.label;
        if (renaming === speaker.key)
          return (
            <RenameField
              key={speaker.key}
              speaker={speaker}
              busy={busy}
              suggestions={nameSuggestions(people, roster, speaker.label)}
              onCancel={() => setRenaming(null)}
              onSave={async (name) => {
                if (
                  await run({
                    type: "speaker_rename",
                    id: session.id,
                    speaker: speaker.key,
                    name,
                  })
                )
                  setRenaming(null);
              }}
            />
          );
        return (
          <div
            className={`person speaker-person ${focused ? "focused" : ""}`}
            key={speaker.key}
          >
            {renderAvatar ? (
              renderAvatar(speaker, tone)
            ) : (
              <Avatar size={34} radius="xl" color={tone}>
                {speaker.label.slice(0, 1)}
              </Avatar>
            )}
            <span className="speaker-person-name">
              <span>{speaker.label}</span>
              <small>
                {speaker.turns} {speaker.turns === 1 ? "turn" : "turns"}
              </small>
            </span>
            {speaker.kind === "you" && (
              <Badge size="xs" color="gray" variant="light">
                YOU
              </Badge>
            )}
            <Menu withinPortal position="bottom-end">
              <Menu.Target>
                <ActionIcon
                  className="speaker-person-menu"
                  size={26}
                  variant="subtle"
                  color="gray"
                  aria-label={`Actions for ${speaker.label}`}
                >
                  <DotsThreeVertical size={16} />
                </ActionIcon>
              </Menu.Target>
              <Menu.Dropdown>
                <Menu.Item
                  leftSection={<FunnelSimple size={15} />}
                  onClick={() => onFilter(focused ? null : speaker.label)}
                >
                  {focused ? "Show everyone" : "Show only their turns"}
                </Menu.Item>
                {speaker.renamable && (
                  <Menu.Item
                    leftSection={<PencilSimple size={15} />}
                    disabled={!editable}
                    onClick={() => setRenaming(speaker.key)}
                  >
                    Rename…
                  </Menu.Item>
                )}
                {roster.length > 1 && (
                  <>
                    <Menu.Label>Merge into</Menu.Label>
                    {roster
                      .filter((other) => other.key !== speaker.key)
                      .map((other) => (
                        <Menu.Item
                          key={other.key}
                          leftSection={<ArrowsMerge size={15} />}
                          disabled={!editable}
                          onClick={() =>
                            void run({
                              type: "speaker_merge",
                              id: session.id,
                              from: speaker.key,
                              into: other.key,
                            })
                          }
                        >
                          {other.label}
                        </Menu.Item>
                      ))}
                  </>
                )}
              </Menu.Dropdown>
            </Menu>
          </div>
        );
      })}
      {session?.speaker_undo && session.kind === "call" && (
        <Tooltip label={session.speaker_undo} withinPortal>
          <Button
            className="speaker-undo"
            variant="subtle"
            color="gray"
            size="compact-xs"
            leftSection={<ArrowCounterClockwise size={13} />}
            disabled={!editable || busy}
            onClick={() => void run({ type: "speaker_undo", id: session.id })}
          >
            Undo speaker change
          </Button>
        </Tooltip>
      )}
    </div>
  );
}
