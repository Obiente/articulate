import { Button, Group, Paper, Stack, Text } from "@mantine/core";
export function VocabularyReview({ review = {}, act }) {
  const run = (action) => void act(action).catch(() => {});
  return (
    <Paper withBorder p="lg" radius="lg" mb="lg">
      <Stack>
        <Group justify="space-between">
          <Text fw={600}>Review vocabulary in your latest dictation</Text>
          <Button
            loading={review.working}
            onClick={() => run({ type: "assort_start" })}
          >
            Review vocabulary
          </Button>
        </Group>
        <Text size="sm" c="dimmed">
          Review up to eight spelling choices against their sentence context.
          Each choice changes only the current dictation. Scores express model
          preference, not confidence.
        </Text>
        <Text role="status">{review.status}</Text>
        {review.candidates?.map((candidate, index) => (
          <Paper key={index} withBorder p="md">
            <Text>{candidate.context}</Text>
            <Text fw={600}>
              {candidate.entry.heard} → {candidate.entry.wanted}
            </Text>
            <Text size="sm">
              Preference score:{" "}
              {review.scores
                ?.find((s) => s.candidate === index)
                ?.replace_score?.toFixed(3) ?? "Unavailable"}
            </Text>
            <Group mt="sm">
              <Button
                onClick={() =>
                  run({ type: "assort_apply", index, saved_spelling: true })
                }
              >
                Use saved spelling
              </Button>
              <Button
                variant="default"
                onClick={() =>
                  run({ type: "assort_apply", index, saved_spelling: false })
                }
              >
                Keep original spelling
              </Button>
            </Group>
          </Paper>
        ))}
        {(review.working || review.candidates) && (
          <Button
            variant="subtle"
            onClick={() => run({ type: "assort_cancel" })}
          >
            Cancel review
          </Button>
        )}
      </Stack>
    </Paper>
  );
}
