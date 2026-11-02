import { memo, useEffect, useMemo, useRef, useState } from "react";
import {
  ActionIcon,
  Button,
  Group,
  Indicator,
  Popover,
  SegmentedControl,
  Stack,
  Text,
  TextInput,
} from "@mantine/core";
import {
  ArrowUpRight,
  Funnel,
  Headphones,
  Info,
  Microphone,
  Notebook,
  PushPin,
} from "@phosphor-icons/react";
import { formatDate, formatTime } from "../../lib/format.js";
import {
  activeFilterCount,
  createSearchScheduler,
  emptyFilters,
  fieldLabels,
  filterProblem,
  groupHits,
  highlightSegments,
  searchFilters,
  searchView,
} from "./search.js";

const kindIcon = (kind) =>
  kind === "call" ? Headphones : kind === "note" ? Notebook : Microphone;
const kindLabel = (kind) =>
  kind === "call" ? "Conversation" : kind === "note" ? "Note" : "Dictation";

// Debounced full-text search state for a library page.
export function useLibrarySearch({ act, search, defaultKind }) {
  const [query, setQuery] = useState(""),
    [filters, setFilters] = useState({ ...emptyFilters, kind: defaultKind }),
    [page, setPage] = useState(defaultKind);
  // Each page starts with its own empty search.
  if (page !== defaultKind) {
    setPage(defaultKind);
    setQuery("");
    setFilters({ ...emptyFilters, kind: defaultKind });
  }
  const actRef = useRef(act);
  actRef.current = act;
  const scheduler = useMemo(
    () =>
      createSearchScheduler({
        run: (text, wanted) =>
          void actRef
            .current({
              type: "history_search",
              query: text,
              filters: wanted,
            })
            .catch(() => {}),
      }),
    [],
  );
  const problem = filterProblem(filters);
  useEffect(() => {
    if (!problem) scheduler.schedule(query, searchFilters(filters));
  }, [query, filters, problem, scheduler]);
  useEffect(() => () => scheduler.cancel(), [scheduler]);
  const view = searchView(search, query, filters);
  return {
    query,
    setQuery,
    filters,
    setFilters,
    problem,
    view,
    filterCount: activeFilterCount(filters, defaultKind),
    clear() {
      setQuery("");
      setFilters({ ...emptyFilters, kind: defaultKind });
    },
  };
}

export function SearchFilters({
  filters,
  setFilters,
  count,
  problem,
  defaultKind,
}) {
  const set = (key) => (event) =>
    setFilters({ ...filters, [key]: event.currentTarget.value });
  return (
    <Popover position="bottom-end" width={320} shadow="md" withinPortal>
      <Popover.Target>
        <Indicator
          disabled={!count}
          label={count}
          size={16}
          offset={4}
          color="mint"
        >
          <ActionIcon
            variant="default"
            size={36}
            aria-label={
              count ? `Search filters, ${count} active` : "Search filters"
            }
          >
            <Funnel size={18} />
          </ActionIcon>
        </Indicator>
      </Popover.Target>
      <Popover.Dropdown>
        <Stack gap="sm">
          <Text size="sm" fw={600}>
            Narrow your search
          </Text>
          <SegmentedControl
            aria-label="Kind of item"
            size="xs"
            fullWidth
            value={filters.kind || "all"}
            onChange={(value) =>
              setFilters({ ...filters, kind: value === "all" ? null : value })
            }
            data={[
              { value: "all", label: "All" },
              { value: "note", label: "Notes" },
              { value: "call", label: "Calls" },
              { value: "dictation", label: "Dictations" },
            ]}
          />
          <Group grow gap="xs">
            <TextInput
              type="date"
              label="From"
              size="xs"
              value={filters.from}
              onChange={set("from")}
            />
            <TextInput
              type="date"
              label="To"
              size="xs"
              value={filters.to}
              onChange={set("to")}
              error={problem}
            />
          </Group>
          <TextInput
            label="Speaker"
            size="xs"
            placeholder="Any speaker"
            value={filters.speaker}
            onChange={set("speaker")}
          />
          <TextInput
            label="Dictated into app"
            size="xs"
            placeholder="For example, outlook.exe"
            value={filters.app}
            onChange={set("app")}
          />
          <Button
            variant="subtle"
            size="xs"
            disabled={!count}
            onClick={() => setFilters({ ...emptyFilters, kind: defaultKind })}
          >
            Clear filters
          </Button>
        </Stack>
      </Popover.Dropdown>
    </Popover>
  );
}

function Highlighted({ text, matches }) {
  const segments = useMemo(
    () => highlightSegments(text, matches),
    [text, matches],
  );
  return segments.map((segment, index) =>
    segment.match ? (
      <mark key={index} className="search-match">
        {segment.text}
      </mark>
    ) : (
      <span key={index}>{segment.text}</span>
    ),
  );
}

const ResultCard = memo(function ResultCard({
  group,
  summary,
  open,
  setPreview,
}) {
  const kind = summary?.kind || group.first.kind;
  const Icon = kindIcon(kind);
  return (
    <button
      className="session-card search-result"
      onClick={() => void open(group.id)}
      onFocus={() => setPreview(group.id)}
      onMouseEnter={() => setPreview(group.id)}
    >
      <div className={`session-icon ${kind}`}>
        <Icon size={23} />
      </div>
      <div className="session-card-copy">
        <div className="session-meta">
          {kindLabel(kind).toUpperCase()}
          <span>{formatDate(group.first.created_ms)}</span>
          {summary?.pinned && (
            <PushPin size={13} weight="fill" aria-label="Pinned" />
          )}
        </div>
        <h2>{summary?.title || group.first.title || "Untitled"}</h2>
        {group.hits.map((hit, index) => (
          <p className="search-excerpt" key={index}>
            <span className="search-field">
              {fieldLabels[hit.field] || "Text"}
              {hit.speaker ? ` · ${hit.speaker}` : ""}
              {hit.start_ms != null ? ` · ${formatTime(hit.start_ms)}` : ""}
            </span>
            <Highlighted text={hit.excerpt} matches={hit.matches} />
          </p>
        ))}
        {group.total > group.hits.length && (
          <div className="session-card-footer">
            <span>
              {group.total - group.hits.length} more{" "}
              {group.total - group.hits.length === 1 ? "match" : "matches"}
            </span>
          </div>
        )}
      </div>
      <ArrowUpRight className="session-open-icon" size={20} />
    </button>
  );
});

// Search results for the library: matches grouped by item.
export function SearchResults({
  view,
  history,
  open,
  setPreview,
  clear,
  onRecovery,
}) {
  const summaries = useMemo(
    () => new Map(history.map((item) => [item.id, item])),
    [history],
  );
  const results = view.results;
  // Items deleted since the search are hidden.
  const groups = useMemo(
    () => groupHits(results?.hits).filter((group) => summaries.has(group.id)),
    [results, summaries],
  );
  return (
    <div className="search-results" aria-busy={view.stale}>
      <div className="section-heading">
        <span>SEARCH RESULTS</span>
        <span>
          {view.stale && !results
            ? "Searching…"
            : `${groups.length} ${groups.length === 1 ? "item" : "items"}`}
        </span>
      </div>
      {view.error && (
        <Text c="red" size="sm" role="alert" mb="sm">
          {view.error}
        </Text>
      )}
      {results && !results.indexed && (
        <div className="search-note" role="status">
          <Info size={16} />
          <span>
            Your library is still being prepared for search. These results cover
            your most recent items.
          </span>
        </div>
      )}
      {results?.indexed && (results.partial || results.more) && (
        <div className="search-note" role="status">
          <Info size={16} />
          <span>
            Showing the first matches. Add a word or a filter to narrow your
            search.
          </span>
        </div>
      )}
      {results?.unreadable_sessions > 0 && (
        <div className="search-note warning" role="status">
          <Info size={16} />
          <span>
            {results.unreadable_sessions}{" "}
            {results.unreadable_sessions === 1
              ? "item couldn't be read and wasn't searched."
              : "items couldn't be read and weren't searched."}
          </span>
          {onRecovery && (
            <Button variant="subtle" size="compact-xs" onClick={onRecovery}>
              Review
            </Button>
          )}
        </div>
      )}
      <div className={view.stale ? "search-stale" : undefined}>
        {groups.map((group) => (
          <ResultCard
            key={group.id}
            group={group}
            summary={summaries.get(group.id)}
            open={open}
            setPreview={setPreview}
          />
        ))}
      </div>
      {results && !view.stale && !groups.length && !view.error && (
        <div className="empty-state">
          <Notebook size={40} weight="duotone" />
          <h2>No matching results</h2>
          <p>Try a different word, remove a filter, or clear your search.</p>
          <Button variant="light" onClick={clear}>
            Clear search
          </Button>
        </div>
      )}
      {results && groups.length > 0 && (
        <Text c="dimmed" size="xs" ta="center" mt="sm">
          Matches ignore case and accents. Use quotes for an exact phrase.
        </Text>
      )}
    </div>
  );
}
