function () {
  // Inspect loaded module exports only. Never invoke the module loader.
  const discoveryDeadline = performance.now() + 1000;
  const stores = new Map();
  const wanted = new Map([
    ['SelectedChannelStore', ['getVoiceChannelId']],
    ['VoiceStateStore', ['getVoiceStatesForChannel']],
    ['SpeakingStore', ['isSpeaking']],
    ['UserStore', ['getUser', 'getCurrentUser']],
    ['GuildMemberStore', ['getMember']],
    ['ChannelStore', ['getChannel']]
  ]);
  const inspect = value => {
    if (!value || typeof value !== 'object') return;
    if (typeof value.addChangeListener !== 'function' || typeof value.removeChangeListener !== 'function' || typeof value.getName !== 'function') return;
    const name = value.getName();
    const methods = wanted.get(name);
    if (methods && methods.every(method => typeof value[method] === 'function')) stores.set(name, value);
  };
  for (const key of Object.keys(this).slice(0, 50000)) {
    if (performance.now() > discoveryDeadline) throw new Error('Voice discovery timed out');
    const module = Object.getOwnPropertyDescriptor(this, key)?.value;
    const exported = module && Object.getOwnPropertyDescriptor(module, 'exports')?.value;
    if (!exported || !['object', 'function'].includes(typeof exported)) continue;
    inspect(exported);
    for (const descriptor of Object.values(Object.getOwnPropertyDescriptors(exported)).slice(0, 128)) {
      let value = descriptor.value;
      // Webpack re-exports use small getters returning a lexical binding.
      if (descriptor.get) {
        const source = Function.prototype.toString.call(descriptor.get);
        if (source.length > 120 || !/^(?:\(\)\s*=>\s*[\w$]+|function\s*\([^)]*\)\s*\{\s*return\s+[\w$]+;?\s*\})$/.test(source)) continue;
        try { value = descriptor.get.call(exported); } catch { continue; }
      }
      inspect(value);
    }
    if (stores.size === wanted.size) break;
  }
  for (const name of wanted.keys()) if (!stores.has(name)) throw new Error('Required voice interface unavailable');
  const selected = stores.get('SelectedChannelStore');
  const voices = stores.get('VoiceStateStore');
  const speaking = stores.get('SpeakingStore');
  const users = stores.get('UserStore');
  const members = stores.get('GuildMemberStore');
  const channels = stores.get('ChannelStore');
  const subscribed = [];
  let queue = [], stopped = false, overflow = false;
  let lease = performance.now() + 4000;
  const sample = () => {
    if (stopped) return;
    try {
      const channelId = selected.getVoiceChannelId() || null;
      const channel = channelId ? channels.getChannel(channelId) : null;
      const localId = users.getCurrentUser()?.id;
      if (channelId && (typeof localId !== 'string' || !/^\d{1,24}$/.test(localId))) throw new Error('Current voice identity unavailable');
      const states = channelId ? Object.values(voices.getVoiceStatesForChannel(channelId)) : [];
      if (states.length > 256) throw new Error('Participant bound exceeded');
      const seen = new Set();
      const participants = states.map(state => {
        const id = state.userId;
        if (typeof id !== 'string' || !/^\d{1,24}$/.test(id) || seen.has(id)) throw new Error('Unexpected voice state');
        seen.add(id);
        const user = users.getUser(id);
        const member = channel?.guild_id ? members.getMember(channel.guild_id, id) : null;
        const name = member?.nick || user?.globalName || user?.username;
        if (typeof name !== 'string' || !name) throw new Error('Display name unavailable');
        return { id, name: name.slice(0, 128), speaking: !!speaking.isSpeaking(id), is_self: id === localId };
      });
      if (queue.length >= 512) { queue = []; overflow = true; }
      queue.push({ time: performance.now(), channel_id: channelId, participants, valid: true });
    } catch {
      queue = [{ time: performance.now(), channel_id: null, participants: [], valid: false }];
    }
  };
  let timer;
  const stop = () => {
    if (stopped) return;
    stopped = true;
    clearInterval(timer);
    for (const store of subscribed) { try { store.removeChangeListener(sample); } catch {} }
    queue = [];
    stores.clear();
  };
  try {
    for (const store of [selected, voices, speaking]) {
      store.addChangeListener(sample);
      subscribed.push(store);
    }
    timer = setInterval(() => {
      if (performance.now() > lease) stop();
      else sample();
    }, 150);
    sample();
  } catch (error) { stop(); throw error; }
  return {
    drain() {
      if (stopped) throw new Error('Voice observer expired');
      lease = performance.now() + 4000;
      sample();
      const result = { now: performance.now(), overflow, samples: queue };
      queue = []; overflow = false;
      return result;
    },
    stop
  };
}
