import { useEffect, useState } from "react";
import {
  Badge,
  Button,
  Divider,
  Group,
  Loader,
  Paper,
  Progress,
  SimpleGrid,
  Stack,
  Text,
  ThemeIcon,
} from "@mantine/core";
import {
  ArrowClockwise,
  ChartBar,
  Microphone,
  ShieldCheck,
  Sparkle,
  Waveform,
} from "@phosphor-icons/react";
import { Empty, Heading, Metric } from "../../components/Page.jsx";
import { formatCount as count } from "../../lib/format.js";

export function InsightsPage({ insights: report, historyLoading, act }) {
  const [refreshing, setRefreshing] = useState(false);
  const refresh = async () => {
    setRefreshing(true);
    try {
      await act({ type: "insights_refresh" });
    } catch {
      /* Root presents the error. */
    } finally {
      setRefreshing(false);
    }
  };
  useEffect(() => {
    act({ type: "insights_refresh" }).catch(() => {});
  }, [act]);
  const apps = report?.apps || [];
  const days = report?.days || [];
  const recentDays = days.slice(-7);
  const maxDayWords = Math.max(1, ...recentDays.map((day) => day.words));
  return (
    <div className="secondary-page">
      <Heading
        title="Insights"
        action={
          <Button
            variant="default"
            leftSection={<ArrowClockwise size={18} />}
            onClick={refresh}
            loading={refreshing}
          >
            Refresh
          </Button>
        }
      >
        See where your voice is saving you effort.
      </Heading>
      {!report ? (
        <Empty
          icon={ChartBar}
          title="Your activity, in perspective"
          action={
            historyLoading ? (
              <Loader size="sm" />
            ) : (
              <Button variant="light" onClick={refresh} loading={refreshing}>
                Load insights
              </Button>
            )
          }
        >
          Insights are calculated from the dictations saved on this device.
        </Empty>
      ) : (
        <>
          <SimpleGrid cols={{ base: 1, sm: 3 }} spacing="lg">
            <Metric
              icon={Microphone}
              label="Words dictated"
              value={count(report.words)}
              description={`${count(report.sessions)} saved dictations`}
            />
            <Metric
              icon={Waveform}
              label="Speaking pace"
              value={
                report.words_per_minute == null
                  ? "—"
                  : `${Math.round(report.words_per_minute)} wpm`
              }
              description={
                report.words_per_minute == null
                  ? "Available after a measured dictation"
                  : "Based on recorded speaking time"
              }
            />
            <Metric
              icon={Sparkle}
              label="Helpful corrections"
              value={count(
                Number(report.dictionary_replacements || 0) +
                  Number(report.cleanup_edits || 0),
              )}
              description={`${count(report.dictionary_replacements)} vocabulary · ${count(report.cleanup_edits)} cleanup`}
            />
          </SimpleGrid>
          {report.sessions === 0 && (
            <Paper withBorder radius="lg" className="secondary-tip">
              <ThemeIcon size={42} variant="light" radius="md">
                <Microphone size={22} />
              </ThemeIcon>
              <div>
                <Text fw={600}>Start with a few words</Text>
                <Text c="dimmed" size="sm">
                  Your first saved dictation will start your activity history
                  here.
                </Text>
              </div>
            </Paper>
          )}
          <div className="secondary-insights-grid">
            <Paper withBorder radius="lg" className="secondary-insight-panel">
              <div className="secondary-panel-title">
                <h2>Where you dictate</h2>
                <Text c="dimmed" size="sm">
                  {count(apps.filter((app) => app.app).length)} apps
                </Text>
              </div>
              {apps.length ? (
                <Stack gap="xl" mt="lg">
                  {apps.map((app, index) => (
                    <div key={app.app || `unknown-${index}`}>
                      <Group justify="space-between" mb={8}>
                        <Text fw={500}>{app.app || "Other dictations"}</Text>
                        <Text c="dimmed" size="sm">
                          {count(app.words)} words
                        </Text>
                      </Group>
                      <Progress
                        size={9}
                        radius="xl"
                        value={
                          report.words
                            ? Math.min(100, (app.words / report.words) * 100)
                            : 0
                        }
                        aria-label={`${app.app || "Other dictations"} share of words`}
                      />
                    </div>
                  ))}
                </Stack>
              ) : (
                <Text c="dimmed" mt="lg">
                  App activity appears when you dictate into a text field.
                </Text>
              )}
            </Paper>
            <Paper withBorder radius="lg" className="secondary-insight-panel">
              <div className="secondary-panel-title">
                <h2>The last 7 days</h2>
                <Badge variant="light">
                  {count(report.current_streak)} day streak
                </Badge>
              </div>
              <Stack gap="md" mt="lg">
                {recentDays.map((day) => (
                  <div className="secondary-day" key={day.date}>
                    <Text c="dimmed" size="sm">
                      {new Date(`${day.date}T12:00:00`).toLocaleDateString(
                        undefined,
                        { weekday: "short" },
                      )}
                    </Text>
                    <Progress
                      size={9}
                      radius="xl"
                      value={(day.words / maxDayWords) * 100}
                      aria-label={`${day.date}: ${count(day.words)} words`}
                    />
                    <Text size="sm" ta="right">
                      {count(day.words)}
                    </Text>
                  </div>
                ))}
              </Stack>
              <Divider my="xl" />
              <Group justify="space-between">
                <Text c="dimmed" size="sm">
                  Longest streak
                </Text>
                <Text fw={500}>{count(report.longest_streak)} days</Text>
              </Group>
              <Group justify="space-between" mt="sm">
                <Text c="dimmed" size="sm">
                  Active days
                </Text>
                <Text fw={500}>{count(report.active_days)}</Text>
              </Group>
            </Paper>
          </div>
          <Text c="dimmed" size="sm" className="secondary-privacy">
            <ShieldCheck size={17} />
            Based on retained dictations on this device. Deleting a dictation
            also removes its activity.
            {report.dates_use_utc ? " Activity dates use UTC." : ""}
          </Text>
        </>
      )}
    </div>
  );
}
