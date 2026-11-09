import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { client, type Snapshot } from "./bridge";

vi.mock("./bridge", () => ({ client: {
  state: vi.fn(), connect: vi.fn(), disconnect: vi.fn(), volume: vi.fn(),
  devices: vi.fn(), subscribe: vi.fn(),
} }));

const initial: Snapshot = {
  revision: 1, version: "0.1.0", username: "Alice", server: "localhost",
  status: "disconnected", audioReady: true, audioError: null, error: null,
  users: [], inputDevices: [], outputDevices: [],
};
const connected: Snapshot = { ...initial, revision: 2, status: "connected", users: [{
  clientId: 7, name: "Alice", volume: 100, isCurrentUser: true,
  latencyMs: 12, isPeerToPeer: false, isSpeaking: false,
}, { clientId: 8, name: "Bob", volume: 100, isCurrentUser: false,
  latencyMs: 5, isPeerToPeer: true, isSpeaking: true,
}] };
let update: (state: Snapshot) => void;
const stop = vi.fn();

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(client.subscribe).mockImplementation(async (callback) => { update = callback; return stop; });
  vi.mocked(client.state).mockResolvedValue(initial);
  vi.mocked(client.connect).mockResolvedValue(connected);
  vi.mocked(client.disconnect).mockResolvedValue({ ...initial, revision: 3 });
  vi.mocked(client.volume).mockResolvedValue({ ...connected, revision: 3 });
  vi.mocked(client.devices).mockResolvedValue(connected);
});
afterEach(cleanup);

async function ready() {
  render(<App />);
  await waitFor(() => expect((screen.getByLabelText("Your name") as HTMLInputElement).value).toBe("Alice"));
}

describe("native client interface", () => {
  it("loads saved settings, connects, renders speaking peers, and disconnects", async () => {
    await ready();
    await userEvent.click(screen.getByRole("button", { name: /Join conversation/ }));
    expect(client.connect).toHaveBeenCalledWith("Alice", "localhost");
    expect(await screen.findByText("Bob")).toBeTruthy();
    expect(screen.getByText("Direct connection · Speaking")).toBeTruthy();
    expect(screen.getByText("5.0")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Disconnect" }));
    expect(await screen.findByLabelText("Your name")).toBeTruthy();
    expect(client.disconnect).toHaveBeenCalledTimes(1);
  });

  it("shows connection failures and lets the user retry", async () => {
    vi.mocked(client.connect).mockRejectedValueOnce("Could not connect to the server");
    await ready();
    await userEvent.click(screen.getByRole("button", { name: /Join conversation/ }));
    expect((await screen.findByRole("alert")).textContent).toContain("Could not connect");
    await userEvent.click(screen.getByRole("button", { name: /Join conversation/ }));
    expect(await screen.findByText("Bob")).toBeTruthy();
  });

  it("keeps the newest state when an older initial snapshot arrives", async () => {
    vi.mocked(client.subscribe).mockImplementation(async (callback) => { callback(connected); update = callback; return stop; });
    render(<App />);
    expect(await screen.findByText("Bob")).toBeTruthy();
    await waitFor(() => expect(client.state).toHaveBeenCalled());
    expect(screen.queryByLabelText("Your name")).toBeNull();
  });

  it("shows device errors and disables connecting", async () => {
    vi.mocked(client.state).mockResolvedValue({ ...initial, audioReady: false, audioError: "Microphone permission denied" });
    await ready();
    expect(screen.getByRole("alert").textContent).toContain("Microphone permission denied");
    expect((screen.getByRole("button", { name: /Join conversation/ }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("sends per-user volume changes and lists devices from native events", async () => {
    vi.mocked(client.state).mockResolvedValue(connected);
    render(<App />);
    const slider = await screen.findByRole("slider", { name: /Bob volume/ });
    fireEvent.change(slider, { target: { value: "150" } });
    await waitFor(() => expect(client.volume).toHaveBeenCalledWith(8, 150));
    await userEvent.click(screen.getByRole("button", { name: "Audio devices" }));
    expect(client.devices).toHaveBeenCalledTimes(1);
    act(() => update({ ...connected, revision: 4, inputDevices: [{ id: "mic", label: "Built-in microphone" }] }));
    expect(screen.getByText("Built-in microphone")).toBeTruthy();
  });

  it("prefills incoming server links without overwriting edits on unrelated events", async () => {
    await ready();
    const server = screen.getByLabelText("Server address") as HTMLInputElement;
    fireEvent.change(server, { target: { value: "my-edit.example" } });
    act(() => update({ ...initial, revision: 2 }));
    expect(server.value).toBe("my-edit.example");
    act(() => update({ ...initial, revision: 3, server: "invited.example" }));
    expect(server.value).toBe("invited.example");
  });

  it("serializes and coalesces rapid volume changes", async () => {
    vi.mocked(client.state).mockResolvedValue(connected);
    let finish!: (state: Snapshot) => void;
    vi.mocked(client.volume).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    render(<App />);
    const slider = await screen.findByRole("slider", { name: /Bob volume/ });
    fireEvent.change(slider, { target: { value: "110" } });
    fireEvent.change(slider, { target: { value: "120" } });
    fireEvent.change(slider, { target: { value: "130" } });
    expect(client.volume).toHaveBeenCalledTimes(1);
    await act(async () => finish({ ...connected, revision: 3 }));
    await waitFor(() => expect(client.volume).toHaveBeenCalledTimes(2));
    expect(client.volume).toHaveBeenLastCalledWith(8, 130);
  });

  it("keeps audio devices collapsed until requested and lets the user close them", async () => {
    await ready();
    const toggle = screen.getByRole("button", { name: "Audio devices" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByRole("region", { name: "Audio devices" })).toBeNull();
    expect(client.devices).not.toHaveBeenCalled();
    await userEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByRole("region", { name: "Audio devices" })).toBeTruthy();
    expect(screen.getByText("No microphones found.")).toBeTruthy();
    await userEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByRole("region", { name: "Audio devices" })).toBeNull();
    expect(client.devices).toHaveBeenCalledTimes(1);
  });

  it("unsubscribes from native events when unmounted", async () => {
    const view = render(<App />);
    await screen.findByLabelText("Your name");
    await waitFor(() => expect(client.state).toHaveBeenCalled());
    view.unmount();
    expect(stop).toHaveBeenCalledTimes(1);
  });
});
