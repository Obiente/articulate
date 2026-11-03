// A participant's Discord picture with initials as the fallback.
//
// Reusable anywhere a participant is shown: pass the participant object from
// `state.discord.participants` or any `{ name, avatar }` pair. Pictures are
// resolved through the controller's cache; while a picture is loading or
// pictures are turned off, the initials badge stays in place.
import { useEffect, useRef, useState } from "react";
import { Avatar } from "@mantine/core";
import { participantAvatar } from "../../bridge.js";
import { createAvatarCache, participantInitials } from "./avatars.js";
import "./participants.css";

const cache = createAvatarCache({ load: participantAvatar });

// Drops every cached picture, for example when pictures are turned off or
// on again; identifiers still on screen are resolved afresh.
export function clearParticipantAvatars() {
  cache.clear();
}

// The URL for an avatar identifier, or null. Updates when it resolves.
export function useParticipantAvatar(uri) {
  const [url, setUrl] = useState(() => (uri ? cache.peek(uri) : null) ?? null);
  const shown = useRef(uri);
  useEffect(() => {
    // A participant whose picture changed leaves the old one behind: it is
    // forgotten so its URL can be freed once nothing else shows it.
    if (shown.current && shown.current !== uri) cache.forget(shown.current);
    shown.current = uri;
    if (!uri) {
      setUrl(null);
      return undefined;
    }
    let active = true;
    const apply = (value) => active && setUrl(value ?? null);
    const unsubscribe = cache.subscribe(uri, apply);
    void cache.resolve(uri).then(apply);
    return () => {
      active = false;
      unsubscribe();
    };
  }, [uri]);
  return url;
}

export function ParticipantAvatar({
  participant,
  size = 32,
  color = "mint",
  speaking = false,
  className = "",
}) {
  const name = participant?.name || "";
  const url = useParticipantAvatar(participant?.avatar || null);
  return (
    <Avatar
      src={url || undefined}
      alt={url ? name : ""}
      title={name}
      size={size}
      radius="xl"
      color={color}
      variant={url ? "default" : "light"}
      className={`participant-avatar ${speaking ? "participant-avatar-speaking" : ""} ${className}`.trim()}
      data-speaking={speaking ? "true" : undefined}
    >
      {participantInitials(name)}
    </Avatar>
  );
}
