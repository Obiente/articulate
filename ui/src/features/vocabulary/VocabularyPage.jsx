import { useState } from "react";
import {
  ActionIcon,
  Badge,
  Button,
  Group,
  Modal,
  Paper,
  SimpleGrid,
  Stack,
  Table,
  Text,
  TextInput,
  ThemeIcon,
  Tooltip,
} from "@mantine/core";
import {
  BookOpen,
  MagnifyingGlass,
  PencilSimple,
  Plus,
  Trash,
} from "@phosphor-icons/react";
import { DeleteDialog, Empty, Heading } from "../../components/Page.jsx";
import { formatCount } from "../../lib/format.js";

const emptyEntry = { heard: "", wanted: "", app: "", cues: "" };

export function VocabularyPage({ dictionary, act }) {
  const [search, setSearch] = useState("");
  const [editor, setEditor] = useState(null);
  const [deletion, setDeletion] = useState(null);
  const [busy, setBusy] = useState(false);
  const entries = dictionary || [];
  const filtered = entries
    .map((entry, index) => ({ ...entry, index }))
    .filter((entry) =>
      `${entry.heard} ${entry.wanted} ${entry.app || ""} ${(entry.cues || []).join(" ")}`
        .toLowerCase()
        .includes(search.toLowerCase()),
    );
  const add = () => setEditor({ ...emptyEntry });
  const save = async (event) => {
    event.preventDefault();
    setBusy(true);
    try {
      await act({ type: "dictionary_save", ...editor });
      setEditor(null);
    } catch {
      /* Root presents the actionable error. */
    } finally {
      setBusy(false);
    }
  };
  const remove = async () => {
    setBusy(true);
    try {
      await act({
        type: "dictionary_delete",
        index: deletion.index,
        expected_heard: deletion.name,
        expected_entry: deletion.original,
      });
      setDeletion(null);
    } catch {
      /* Root presents the error. */
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="secondary-page">
      <Heading
        title="Vocabulary"
        action={
          <Button leftSection={<Plus size={18} />} onClick={add}>
            Add a word
          </Button>
        }
      >
        Your names, terms, and spellings. Remembered wherever you dictate.
      </Heading>
      <Paper className="secondary-tip" withBorder radius="lg">
        <ThemeIcon size={42} radius="md" variant="light">
          <BookOpen size={23} />
        </ThemeIcon>
        <div>
          <Text fw={600}>Make it sound like you</Text>
          <Text c="dimmed" size="sm">
            Add an unusual name or a phrase you often correct. Adding the same
            correction updates its context.
          </Text>
        </div>
      </Paper>
      <div className="secondary-list-toolbar">
        <TextInput
          aria-label="Search vocabulary"
          placeholder="Find a word or spelling"
          leftSection={<MagnifyingGlass size={18} />}
          value={search}
          onChange={(event) => setSearch(event.currentTarget.value)}
        />
        <Text c="dimmed" size="sm">
          {formatCount(entries.length)}{" "}
          {entries.length === 1 ? "word" : "words"}
        </Text>
      </div>
      {filtered.length ? (
        <Paper withBorder radius="lg" className="secondary-table">
          <Table.ScrollContainer minWidth={610}>
            <Table verticalSpacing="lg" horizontalSpacing="lg">
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>When you say</Table.Th>
                  <Table.Th>Write instead</Table.Th>
                  <Table.Th>Where it works</Table.Th>
                  <Table.Th>
                    <span className="secondary-sr-only">Actions</span>
                  </Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {filtered.map((entry) => (
                  <Table.Tr key={entry.index}>
                    <Table.Td>
                      <Text fw={500}>{entry.heard}</Text>
                      {entry.enabled === false && (
                        <Badge size="xs" variant="light" color="gray">
                          Off
                        </Badge>
                      )}
                    </Table.Td>
                    <Table.Td>
                      <Text className="secondary-accent">{entry.wanted}</Text>
                      {entry.cues?.length > 0 && (
                        <Text c="dimmed" size="xs" mt={5}>
                          Near: {entry.cues.join(", ")}
                        </Text>
                      )}
                    </Table.Td>
                    <Table.Td>
                      <Badge variant="light" color="gray">
                        {entry.app || "All apps"}
                      </Badge>
                    </Table.Td>
                    <Table.Td>
                      <Group gap={6} justify="flex-end" wrap="nowrap">
                        <Tooltip label="Edit word">
                          <ActionIcon
                            size={38}
                            variant="subtle"
                            aria-label={`Edit ${entry.heard}`}
                            onClick={() =>
                              setEditor({
                                heard: entry.heard,
                                wanted: entry.wanted,
                                app: entry.app || "",
                                cues: (entry.cues || []).join(", "),
                                index: entry.index,
                                expected_heard: entry.heard,
                                expected_entry: structuredClone(
                                  entries[entry.index],
                                ),
                              })
                            }
                          >
                            <PencilSimple size={19} />
                          </ActionIcon>
                        </Tooltip>
                        <Tooltip label="Delete word">
                          <ActionIcon
                            size={38}
                            variant="subtle"
                            color="gray"
                            aria-label={`Delete ${entry.heard}`}
                            onClick={() =>
                              setDeletion({
                                index: entry.index,
                                name: entry.heard,
                                original: structuredClone(entries[entry.index]),
                              })
                            }
                          >
                            <Trash size={19} />
                          </ActionIcon>
                        </Tooltip>
                      </Group>
                    </Table.Td>
                  </Table.Tr>
                ))}
              </Table.Tbody>
            </Table>
          </Table.ScrollContainer>
        </Paper>
      ) : (
        <Empty
          icon={BookOpen}
          title={search ? "No matching words" : "A vocabulary that knows you"}
          action={
            !search && (
              <Button
                variant="light"
                leftSection={<Plus size={17} />}
                onClick={add}
              >
                Add your first word
              </Button>
            )
          }
        >
          {search
            ? "Try a different word, spelling, or app."
            : "Give names, acronyms, and specialist terms the spelling they deserve."}
        </Empty>
      )}
      <Modal
        opened={Boolean(editor)}
        onClose={() => !busy && setEditor(null)}
        title={editor?.index == null ? "Add a word" : "Edit word"}
        centered
        size="lg"
        radius="lg"
      >
        <form onSubmit={save}>
          <Stack gap="lg">
            <SimpleGrid cols={{ base: 1, sm: 2 }}>
              <TextInput
                label="When you say"
                placeholder="For example, articulate"
                required
                autoFocus
                value={editor?.heard || ""}
                onChange={(event) =>
                  setEditor({ ...editor, heard: event.currentTarget.value })
                }
              />
              <TextInput
                label="Write instead"
                placeholder="Articulate"
                required
                value={editor?.wanted || ""}
                onChange={(event) =>
                  setEditor({ ...editor, wanted: event.currentTarget.value })
                }
              />
            </SimpleGrid>
            <TextInput
              label="App"
              description="Leave empty to use this spelling in every app."
              placeholder="All apps, or an app such as discord.exe"
              value={editor?.app || ""}
              onChange={(event) =>
                setEditor({ ...editor, app: event.currentTarget.value })
              }
            />
            <TextInput
              label="Context words"
              description="Optional. Use this spelling near these words in the same sentence."
              placeholder="Separate words with commas"
              value={editor?.cues || ""}
              onChange={(event) =>
                setEditor({ ...editor, cues: event.currentTarget.value })
              }
            />
            <Group justify="flex-end" mt="sm">
              <Button
                variant="default"
                onClick={() => setEditor(null)}
                disabled={busy}
              >
                Cancel
              </Button>
              <Button type="submit" loading={busy}>
                Save word
              </Button>
            </Group>
          </Stack>
        </form>
      </Modal>
      <DeleteDialog
        item={deletion}
        noun="word"
        onClose={() => !busy && setDeletion(null)}
        onConfirm={remove}
        busy={busy}
      />
    </div>
  );
}
