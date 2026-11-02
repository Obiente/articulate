// The current voice channel's members with pictures and speaking activity.
import { Text } from "@mantine/core";
import { speakerColors } from "../calls/speakers.js";
import { ParticipantAvatar } from "./ParticipantAvatar.jsx";

export function DiscordParticipants({ participants, inVoice }) {
  if (!participants?.length) {
    return (
      <Text c="dimmed" size="sm" pb="lg">
        {inVoice
          ? "No other participants yet."
          : "Join a voice channel to see who is in the call."}
      </Text>
    );
  }
  return (
    <ul className="participant-list" aria-label="Voice channel participants">
      {participants.map((participant, index) => (
        <li
          key={participant.id}
          className="participant-row"
          data-speaking={participant.speaking ? "true" : "false"}
        >
          <ParticipantAvatar
            participant={participant}
            speaking={participant.speaking}
            color={speakerColors[index % speakerColors.length]}
          />
          <span className="participant-row-name">
            {participant.name}
            {participant.is_self && (
              <Text component="span" c="dimmed" size="sm">
                {" "}
                (you)
              </Text>
            )}
          </span>
          <span
            className="participant-row-state"
            data-speaking={participant.speaking ? "true" : "false"}
          >
            {participant.speaking && (
              <span className="participant-pulse" aria-hidden="true" />
            )}
            {participant.speaking ? "Speaking" : "Quiet"}
          </span>
        </li>
      ))}
    </ul>
  );
}
