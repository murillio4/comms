import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { client, type Snapshot, type User } from "./bridge";

function Volume({ user, change }: { user: User; change: (volume: number) => Promise<void> }) {
  const [volume, setVolume] = useState(user.volume);
  const pending = useRef<number | null>(null);
  const saving = useRef(false);
  useEffect(() => { if (!saving.current) setVolume(user.volume); }, [user.volume]);
  // Coalesce slider motion and serialize writes to prevent older volume writes
  // from overtaking newer ones. Audio samples stay in the native workers.
  async function update(value: number) {
    setVolume(value); pending.current = value;
    if (saving.current) return;
    saving.current = true;
    try {
      while (pending.current !== null) {
        const next = pending.current; pending.current = null;
        await change(next);
      }
    } finally { saving.current = false; }
  }
  return <label className="volume">
    <span>{user.isCurrentUser ? "Headphone volume" : `${user.name} volume`}</span>
    <input type="range" min="0" max="400" step="2" value={volume}
      onChange={(event) => void update(Number(event.target.value))} />
    <output>{volume}%</output>
  </label>;
}

export default function App() {
  const [state, setState] = useState<Snapshot | null>(null);
  const [username, setUsername] = useState("");
  const [server, setServer] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [showDevices, setShowDevices] = useState(false);
  const initialized = useRef(false);
  const revision = useRef(-1);
  const nativeServer = useRef<string | null>(null);
  const accept = useCallback((next: Snapshot) => {
    if (next.revision < revision.current) return;
    revision.current = next.revision; setState(next);
    if (nativeServer.current !== next.server) {
      nativeServer.current = next.server; setServer(next.server);
    }
    if (!initialized.current) {
      setUsername(next.username); initialized.current = true;
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    async function start() {
      try {
        // Listen before fetching state; revisions reject stale snapshots.
        const stop = await client.subscribe((next) => { if (!disposed) accept(next); });
        if (disposed) { stop(); return; }
        unlisten = stop;
        const initial = await client.state();
        if (!disposed) accept(initial);
      } catch (cause) { if (!disposed) setError(String(cause)); }
    }
    void start();
    return () => { disposed = true; unlisten?.(); };
  }, [accept]);

  async function run(action: () => Promise<Snapshot>, lock = false) {
    if (lock) setBusy(true);
    setError(null);
    try { accept(await action()); }
    catch (cause) { setError(String(cause)); }
    finally { if (lock) setBusy(false); }
  }
  function connect(event: FormEvent) {
    event.preventDefault();
    void run(() => client.connect(username.trim(), server.trim()), true);
  }
  const connected = state?.status === "connected";
  const transitioning = busy || state?.status === "connecting" || state?.status === "disconnecting";
  const visibleError = error || state?.audioError || state?.error;

  return <main className="app">
    <header className="masthead">
      <div className="brand"><span className="brand-mark" aria-hidden="true">◖◗</span><span>comms<span className="brand-dot">.</span></span></div>
      <span className={`status ${connected ? "online" : ""}`} role="status"><i />{state?.status ?? "Starting"}</span>
    </header>
    <section className="intro">
      <p className="eyebrow">A little closer.</p>
      <h1>{connected ? "You’re in good company." : "Less delay. More conversation."}</h1>
      <p>{connected ? `Connected to ${state.server}` : "A quiet place for your voice. Connect to a server and join the conversation."}</p>
    </section>
    {visibleError && <div className="error" role="alert">{visibleError}</div>}
    {connected ? <section className="panel" aria-label="Voice session">
      <div className="panel-heading"><div><p className="eyebrow">Voice room</p><h2>In the conversation <span className="count">{state.users.length}</span></h2></div>
        <button className="secondary" disabled={transitioning} onClick={() => void run(client.disconnect, true)}>Disconnect</button></div>
      <ul className="users">{state.users.map((user) => <li key={user.clientId} className={user.isSpeaking ? "speaking" : ""}>
        <div className="user-line">
          <span className="avatar" aria-hidden="true">{user.name.slice(0, 1).toLocaleUpperCase()}</span>
          <div className="user-name"><strong>{user.name}</strong><span>{user.isCurrentUser ? "You" : user.isPeerToPeer ? "Direct connection" : "Via server"}{user.isSpeaking ? " · Speaking" : ""}</span></div>
          <span className="latency" title={user.isCurrentUser ? "One-way latency to server" : "Estimated one-way latency to participant"}>{user.latencyMs.toFixed(1)} <small>ms</small></span>
        </div>
        <Volume user={user} change={(volume) => run(() => client.volume(user.clientId, volume))} />
      </li>)}</ul>
    </section> : <section className="panel connect-panel">
      <p className="eyebrow">Make yourself heard</p><h2>Join a conversation</h2>
      <form onSubmit={connect}>
        <label htmlFor="username">Your name</label>
        <input id="username" name="username" autoComplete="nickname" required value={username} disabled={transitioning}
          onChange={(event) => setUsername(event.target.value)} placeholder="What should we call you?" />
        <label htmlFor="server">Server address</label>
        <input id="server" name="server" autoCapitalize="none" autoCorrect="off" spellCheck={false} required value={server}
          disabled={transitioning} onChange={(event) => setServer(event.target.value)} placeholder="voice.example.com" aria-describedby="server-hint" />
        <p id="server-hint" className="hint">Ask your friends for their server’s hostname. Comms uses port 18847.</p>
        <button className="primary" type="submit" disabled={transitioning || !state?.audioReady || !username.trim() || !server.trim()}>
          {transitioning ? "Connecting…" : "Join conversation"}<span aria-hidden="true">↗</span>
        </button>
      </form>
      <p className="audio-status" role="status"><i className={state?.audioReady ? "ready" : ""} />{state?.audioReady ? "Native audio ready" : state?.audioError ? "Audio unavailable" : "Preparing audio…"}</p>
    </section>}
    <footer><span>Voice, with room to breathe.</span><button className="text-button" disabled={!state?.audioReady}
      onClick={() => { setShowDevices(!showDevices); if (!showDevices) void run(client.devices); }}>Audio devices</button><span>v{state?.version ?? "…"}</span></footer>
    {showDevices && state && <section className="panel devices" aria-label="Audio devices">
      <h2>Audio devices</h2><p>Comms uses your system’s default input and output. Restart Comms after changing devices.</p>
      <h3>Microphones</h3><ul>{state.inputDevices.map((device, index) => <li key={`${device.id}-${index}`}>{device.label}</li>)}</ul>
      <h3>Speakers & headphones</h3><ul>{state.outputDevices.map((device, index) => <li key={`${device.id}-${index}`}>{device.label}</li>)}</ul>
    </section>}
  </main>;
}
