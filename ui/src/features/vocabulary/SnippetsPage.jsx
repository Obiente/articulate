import { useState } from "react";
import {
  ActionIcon,
  Badge,
  Button,
  Group,
  Modal,
  Paper,
  Stack,
  Text,
  Textarea,
  TextInput,
  ThemeIcon,
  Tooltip,
} from "@mantine/core";
import {
  MagnifyingGlass,
  PencilSimple,
  Plus,
  Scissors,
  Trash,
} from "@phosphor-icons/react";
import { DeleteDialog, Empty, Heading } from "../../components/Page.jsx";
import { formatCount } from "../../lib/format.js";

const emptySnippet = { trigger: "", expansion: "", app: "" };

export function SnippetsPage({ macros, act }) {
  const [search, setSearch] = useState("");
  const [editor, setEditor] = useState(null);
  const [deletion, setDeletion] = useState(null);
  const [busy, setBusy] = useState(false);
  const snippets = macros || [];
  const filtered = snippets
    .map((snippet, index) => ({ ...snippet, index }))
    .filter((snippet) =>
      `${snippet.trigger} ${snippet.expansion} ${snippet.app || ""}`
        .toLowerCase()
        .includes(search.toLowerCase()),
    );
  const add = () => setEditor({ ...emptySnippet });
  const save = async (event) => {
    event.preventDefault();
    setBusy(true);
    try {
      await act({ type: "macro_save", ...editor });
      setEditor(null);
    } catch {
      /* Root presents the error. */
    } finally {
      setBusy(false);
    }
  };
  const remove = async () => {
    setBusy(true);
    try {
      await act({
        type: "macro_delete",
        index: deletion.index,
        expected_trigger: deletion.name,
        expected_macro: deletion.original,
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
        title="Snippets"
        action={
          <Button leftSection={<Plus size={18} />} onClick={add}>
            New snippet
          </Button>
        }
      >
        Say a little. Write a lot. Keep your everyday phrases within reach.
      </Heading>
      <Paper className="secondary-tip" withBorder radius="lg">
        <ThemeIcon size={42} radius="md" variant="light">
          <Scissors size={23} />
        </ThemeIcon>
        <div>
          <Text fw={600}>Your words, on cue</Text>
          <Text c="dimmed" size="sm">
            Say “bang” followed by a snippet name to insert its text. Useful for
            signatures, addresses, and familiar replies.
          </Text>
        </div>
      </Paper>
      <div className="secondary-list-toolbar">
        <TextInput
          aria-label="Search snippets"
          placeholder="Find a snippet"
          leftSection={<MagnifyingGlass size={18} />}
          value={search}
          onChange={(event) => setSearch(event.currentTarget.value)}
        />
        <Text c="dimmed" size="sm">
          {formatCount(snippets.length)}{" "}
          {snippets.length === 1 ? "snippet" : "snippets"}
        </Text>
      </div>
      {filtered.length ? (
        <Stack gap="sm">
          {filtered.map((snippet) => (
            <Paper
              withBorder
              radius="lg"
              className="secondary-snippet"
              key={snippet.index}
            >
              <div className="secondary-snippet-top">
                <Group gap="sm">
                  <Text fw={600}>{snippet.trigger}</Text>
                  <Badge variant="light" color="gray">
                    {snippet.app || "All apps"}
                  </Badge>
                  {snippet.enabled === false && (
                    <Badge variant="light" color="gray">
                      Off
                    </Badge>
                  )}
                </Group>
                <Group gap={6} wrap="nowrap">
                  <Tooltip label="Edit snippet">
                    <ActionIcon
                      variant="subtle"
                      size={38}
                      aria-label={`Edit ${snippet.trigger}`}
                      onClick={() =>
                        setEditor({
                          trigger: snippet.trigger,
                          expansion: snippet.expansion,
                          app: snippet.app || "",
                          index: snippet.index,
                          expected_trigger: snippet.trigger,
                          expected_macro: structuredClone(
                            snippets[snippet.index],
                          ),
                        })
                      }
                    >
                      <PencilSimple size={19} />
                    </ActionIcon>
                  </Tooltip>
                  <Tooltip label="Delete snippet">
                    <ActionIcon
                      variant="subtle"
                      color="gray"
                      size={38}
                      aria-label={`Delete ${snippet.trigger}`}
                      onClick={() =>
                        setDeletion({
                          index: snippet.index,
                          name: snippet.trigger,
                          original: structuredClone(snippets[snippet.index]),
                        })
                      }
                    >
                      <Trash size={19} />
                    </ActionIcon>
                  </Tooltip>
                </Group>
              </div>
              <Text className="secondary-snippet-text" lineClamp={4}>
                {snippet.expansion}
              </Text>
              <Text c="dimmed" size="sm" mt="md">
                Say{" "}
                <span className="secondary-accent">
                  “bang {snippet.trigger}”
                </span>
              </Text>
            </Paper>
          ))}
        </Stack>
      ) : (
        <Empty
          icon={Scissors}
          title={
            search
              ? "No matching snippets"
              : "Skip the words you type again and again"
          }
          action={
            !search && (
              <Button
                variant="light"
                leftSection={<Plus size={17} />}
                onClick={add}
              >
                Create a snippet
              </Button>
            )
          }
        >
          {search
            ? "Try another name or a word from the snippet."
            : "Turn a short spoken cue into a complete reply, signature, or template."}
        </Empty>
      )}
      <Modal
        opened={Boolean(editor)}
        onClose={() => !busy && setEditor(null)}
        title={editor?.index == null ? "New snippet" : "Edit snippet"}
        centered
        size="lg"
        radius="lg"
      >
        <form onSubmit={save}>
          <Stack gap="lg">
            <TextInput
              label="Snippet name"
              description="One to four words. Say “bang” before this name when dictating."
              placeholder="For example, my signature"
              required
              autoFocus
              value={editor?.trigger || ""}
              onChange={(event) =>
                setEditor({ ...editor, trigger: event.currentTarget.value })
              }
            />
            <Textarea
              label="Text to insert"
              placeholder="Write the text you want to use…"
              required
              autosize
              minRows={6}
              maxRows={12}
              value={editor?.expansion || ""}
              onChange={(event) =>
                setEditor({ ...editor, expansion: event.currentTarget.value })
              }
            />
            <TextInput
              label="App"
              description="Leave empty to use this snippet in every app."
              placeholder="All apps, or an app such as discord.exe"
              value={editor?.app || ""}
              onChange={(event) =>
                setEditor({ ...editor, app: event.currentTarget.value })
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
                Save snippet
              </Button>
            </Group>
          </Stack>
        </form>
      </Modal>
      <DeleteDialog
        item={deletion}
        noun="snippet"
        onClose={() => !busy && setDeletion(null)}
        onConfirm={remove}
        busy={busy}
      />
    </div>
  );
}
