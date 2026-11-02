import { useState } from "react";
import {
  ActionIcon,
  Avatar,
  Badge,
  Button,
  Group,
  Modal,
  Paper,
  Text,
  TextInput,
} from "@mantine/core";
import {
  Check,
  Headphones,
  PencilSimple,
  Trash,
  UserPlus,
  X,
} from "@phosphor-icons/react";
import { SettingRow, SettingsSection } from "../../components/SettingRow.jsx";
import { keepAudioDescription } from "./ImportAudio.jsx";
import "./calls.css";

function PersonRow({ person, controls, renderAvatar }) {
  const [editing, setEditing] = useState(false),
    [name, setName] = useState(person.name);
  const save = async () => {
    if (name.trim() && name.trim() !== person.name)
      await controls.action("person_save", { id: person.id, name });
    setEditing(false);
  };
  return (
    <div className="known-person">
      {renderAvatar ? (
        renderAvatar(person)
      ) : (
        <Avatar size={30} radius="xl" color="mint">
          {person.name.slice(0, 1)}
        </Avatar>
      )}
      {editing ? (
        <TextInput
          size="xs"
          autoFocus
          aria-label={`New name for ${person.name}`}
          value={name}
          onChange={(event) => setName(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") void save();
            if (event.key === "Escape") setEditing(false);
          }}
        />
      ) : (
        <span className="known-person-name">
          <strong>{person.name}</strong>
          {person.source === "discord" && (
            <small>
              Discord
              {person.discord_name && person.discord_name !== person.name
                ? ` · ${person.discord_name}`
                : ""}
            </small>
          )}
        </span>
      )}
      <Group gap={4} wrap="nowrap">
        {editing ? (
          <>
            <ActionIcon
              size={28}
              variant="light"
              aria-label="Save name"
              onClick={() => void save()}
            >
              <Check size={14} />
            </ActionIcon>
            <ActionIcon
              size={28}
              variant="subtle"
              color="gray"
              aria-label="Cancel"
              onClick={() => setEditing(false)}
            >
              <X size={14} />
            </ActionIcon>
          </>
        ) : (
          <>
            <ActionIcon
              size={28}
              variant="subtle"
              color="gray"
              aria-label={`Rename ${person.name}`}
              disabled={controls.busy}
              onClick={() => {
                setName(person.name);
                setEditing(true);
              }}
            >
              <PencilSimple size={14} />
            </ActionIcon>
            <ActionIcon
              size={28}
              variant="subtle"
              color="red"
              aria-label={`Forget ${person.name}`}
              disabled={controls.busy}
              onClick={() =>
                void controls.action("person_delete", { id: person.id })
              }
            >
              <Trash size={14} />
            </ActionIcon>
          </>
        )}
      </Group>
    </div>
  );
}

// Conversation preferences: keeping audio and the people you have named.
export function ConversationSettings({ controls, renderAvatar }) {
  const { state } = controls;
  const people = state.people || [];
  const [adding, setAdding] = useState(""),
    [clearing, setClearing] = useState(false);
  return (
    <SettingsSection
      id="settings-conversations"
      icon={Headphones}
      title="Conversations"
    >
      <Paper withBorder radius="lg" className="secondary-settings-card">
        <SettingRow
          title="Keep call audio"
          description={`Off unless you choose it. You can also decide for each recording. ${keepAudioDescription} Backups leave audio out unless you include it.`}
        >
          {controls.toggle("keep_call_audio", "Keep call audio by default")}
        </SettingRow>
        <SettingRow
          title="Known people"
          description="Names you give speakers are offered when you rename a speaker. Discord participants are remembered by their Discord account, so later calls show the name you chose. Voices are not recognized: the speaker model only tells voices apart within one recording, so you name them for each conversation."
        >
          <Badge variant="light" color="gray">
            {people.length} {people.length === 1 ? "person" : "people"}
          </Badge>
        </SettingRow>
        {state.people_error && (
          <Text c="red" size="sm" mb="sm">
            {state.people_error}
          </Text>
        )}
        <div className="known-people">
          {people.map((person) => (
            <PersonRow
              key={person.id}
              person={person}
              controls={controls}
              renderAvatar={renderAvatar}
            />
          ))}
          <form
            className="known-person-add"
            onSubmit={async (event) => {
              event.preventDefault();
              if (
                adding.trim() &&
                (await controls.action("person_save", { name: adding }))
              )
                setAdding("");
            }}
          >
            <TextInput
              size="sm"
              placeholder="Add a name"
              aria-label="Add a name"
              value={adding}
              onChange={(event) => setAdding(event.currentTarget.value)}
            />
            <Button
              type="submit"
              size="sm"
              variant="light"
              leftSection={<UserPlus size={15} />}
              disabled={!adding.trim() || controls.busy}
            >
              Add
            </Button>
            {people.length > 0 && (
              <Button
                size="sm"
                variant="subtle"
                color="red"
                disabled={controls.busy}
                onClick={() => setClearing(true)}
              >
                Forget everyone
              </Button>
            )}
          </form>
        </div>
      </Paper>
      <Modal
        opened={clearing}
        onClose={() => setClearing(false)}
        title="Forget everyone?"
        centered
      >
        <Text c="dimmed">
          The list of known people is removed from this device. Names already in
          saved conversations stay as they are.
        </Text>
        <Group justify="flex-end" mt="xl">
          <Button variant="default" onClick={() => setClearing(false)}>
            Cancel
          </Button>
          <Button
            color="red"
            loading={controls.pending === "people_clear"}
            onClick={async () => {
              if (await controls.action("people_clear")) setClearing(false);
            }}
          >
            Forget everyone
          </Button>
        </Group>
      </Modal>
    </SettingsSection>
  );
}
