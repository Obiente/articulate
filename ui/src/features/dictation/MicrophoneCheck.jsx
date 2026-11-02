import { Alert, Button, Progress, Stack, Text } from "@mantine/core";
export function MicrophoneCheck({ check = {}, onTest, disabled = false }) {
  const result = check.result;
  return (
    <Stack gap="sm">
      <Button
        variant="default"
        loading={check.running}
        disabled={disabled}
        onClick={onTest}
      >
        Test microphone
      </Button>
      {check.running && <Text role="status">Speak for three seconds </Text>}
      {check.error && (
        <Alert color="red" role="alert">
          {check.error}
        </Alert>
      )}
      {result && (
        <Stack gap="xs" aria-live="polite">
          <Text size="sm">{result.device}</Text>
          <Progress
            aria-label="Microphone peak level"
            value={Math.min(100, result.peak * 100)}
          />
          <Text size="sm">
            Peak {Math.round(result.peak * 100)}% Average{" "}
            {Math.round(result.rms * 100)}%
          </Text>
          {result.silent && (
            <Alert color="yellow">
              No sound detected. Check mute and microphone access.
            </Alert>
          )}
          {result.clipping_ratio > 0 && (
            <Alert color="yellow">
              Audio is clipping. Lower microphone gain or move farther away.
            </Alert>
          )}
        </Stack>
      )}
    </Stack>
  );
}
