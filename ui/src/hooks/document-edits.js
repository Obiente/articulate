import { prepareAction } from "../actions.js";
export async function flushPatches(pending, send, invalid) {
  while (pending.size) {
    for (const [id, patch] of [...pending.entries()]) {
      let command;
      try {
        command = prepareAction({ type: "session_patch", id, ...patch });
      } catch (error) {
        if (pending.get(id) === patch) pending.delete(id);
        invalid(id, error);
        continue;
      }
      await send(command);
      if (pending.get(id) === patch) pending.delete(id);
    }
  }
}
