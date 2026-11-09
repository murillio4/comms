import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { client, type Snapshot, type User } from "./bridge";
import { AudioLines, ChevronDown, Headphones, LogOut, Mic, Volume2 } from "lucide-react";
import { Badge } from "./components/ui/badge";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";

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
      <div className="brand"><AudioLines aria-hidden="true" /><h1>Comms</h1></div>
      <Badge variant="outline" className="status" role="status">
        <i className={connected ? "online" : ""} />{state?.status ?? "Starting"}
      </Badge>
    </header>

    <div className="content">
      {visibleError && <div className="error" role="alert">{visibleError}</div>}
      {connected ? <section className="session" aria-label="Voice session">
        <div className="session-heading">
          <div className="session-title"><h2>Voice room</h2><p title={state.server}>{state.server}</p></div>
          <Button variant="outline" size="sm" disabled={transitioning} onClick={() => void run(client.disconnect, true)}>
            <LogOut aria-hidden="true" />Disconnect
          </Button>
        </div>
        <div className="list-heading"><span>Participants</span><span>{state.users.length}</span></div>
        <ul className="users">{state.users.map((user) => <li key={user.clientId} className={user.isSpeaking ? "speaking" : ""}>
          <div className="user-line">
            <span className="avatar" aria-hidden="true">{user.name.slice(0, 1).toLocaleUpperCase()}</span>
            <div className="user-name"><strong>{user.name}{user.isCurrentUser && <span className="you">you</span>}</strong>
              <span>{user.isCurrentUser ? "You" : user.isPeerToPeer ? "Direct connection" : "Via server"}{user.isSpeaking ? " · Speaking" : ""}</span>
            </div>
            <span className="latency" title={user.isCurrentUser ? "One-way latency to server" : "Estimated one-way latency to participant"}>{user.latencyMs.toFixed(1)} <small>ms</small></span>
          </div>
          <Volume user={user} change={(volume) => run(() => client.volume(user.clientId, volume))} />
        </li>)}</ul>
        {state.users.length === 0 && <p className="empty-state">Waiting for participants…</p>}
      </section> : <section className="connect-panel" aria-labelledby="connect-heading">
        <div className="section-heading"><h2 id="connect-heading">Join a conversation</h2><p>Enter your name and server to get started.</p></div>
        <form onSubmit={connect}>
          <div className="field"><label htmlFor="username">Your name</label>
            <Input id="username" name="username" autoComplete="nickname" required value={username} disabled={!state || transitioning}
              onChange={(event) => setUsername(event.target.value)} placeholder="Your name" />
          </div>
          <div className="field"><label htmlFor="server">Server address</label>
            <Input id="server" name="server" autoCapitalize="none" autoCorrect="off" spellCheck={false} required value={server}
              disabled={!state || transitioning} onChange={(event) => setServer(event.target.value)} placeholder="voice.example.com" aria-describedby="server-hint" />
            <p id="server-hint" className="hint">Hostname or IP · port 18847</p>
          </div>
          <Button className="join-button" type="submit" disabled={transitioning || !state?.audioReady || !username.trim() || !server.trim()}>
            <Headphones aria-hidden="true" />{transitioning ? "Connecting…" : "Join conversation"}
          </Button>
        </form>
      </section>}
    </div>

    <footer className="footer">
      <span className="audio-status" role="status"><i className={state?.audioReady ? "online" : ""} />{state?.audioReady ? "Audio ready" : state?.audioError ? "Audio unavailable" : "Preparing audio…"}</span>
      <Button variant="ghost" size="sm" className="devices-toggle" disabled={!state?.audioReady}
        aria-expanded={showDevices} aria-controls="audio-devices"
        onClick={() => { setShowDevices(!showDevices); if (!showDevices) void run(client.devices); }}>
        Audio devices<ChevronDown className={showDevices ? "rotate-180" : ""} aria-hidden="true" />
      </Button>
    </footer>
    {showDevices && state && <section id="audio-devices" className="devices" aria-label="Audio devices">
      <h2>Audio devices</h2><p>Uses your system defaults. Restart Comms after changing devices.</p>
      <h3><Mic aria-hidden="true" />Microphones</h3>
      <ul>{state.inputDevices.map((device, index) => <li key={`${device.id}-${index}`}>{device.label}</li>)}</ul>
      {state.inputDevices.length === 0 && <p>No microphones found.</p>}
      <h3><Volume2 aria-hidden="true" />Speakers & headphones</h3>
      <ul>{state.outputDevices.map((device, index) => <li key={`${device.id}-${index}`}>{device.label}</li>)}</ul>
      {state.outputDevices.length === 0 && <p>No output devices found.</p>}
    </section>}
  </main>;
}
