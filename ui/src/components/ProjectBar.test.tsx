import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ProjectBar } from "./ProjectBar";

const { invoke, homeDir } = vi.hoisted(() => ({
  invoke: vi.fn<(command: string, args: { options: { directory: boolean; defaultPath: string } }) => Promise<unknown>>(),
  homeDir: vi.fn(() => Promise.resolve("/home/me/")),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/path", () => ({ homeDir }));

/** Expect the folder dialog to have been asked for, starting at defaultPath. */
function expectDialogAt(defaultPath: string) {
  expect(invoke).toHaveBeenCalledWith("plugin:dialog|open", { options: { directory: true, defaultPath } });
}

describe("the project bar's Choose…", () => {
  beforeEach(() => {
    invoke.mockReset();
  });

  it("opens the folder picked in the system's folder dialog, which starts at the open project", async () => {
    invoke.mockResolvedValue("/home/me/picked");
    const onOpen = vi.fn();
    render(<ProjectBar project={{ cwd: "~/proj" }} onOpen={onOpen} onCheckSetup={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Choose…" }));

    await waitFor(() => expect(onOpen).toHaveBeenCalledWith({ cwd: "/home/me/picked" }));
    expectDialogAt("/home/me/proj");
    expect(screen.getByLabelText("Project folder")).toHaveValue("/home/me/picked");
  });

  it("starts at the home directory with no project open, and opens nothing when cancelled", async () => {
    invoke.mockResolvedValue(null);
    const onOpen = vi.fn();
    render(<ProjectBar project={{ cwd: "" }} onOpen={onOpen} onCheckSetup={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Choose…" }));

    await waitFor(() => expectDialogAt("/home/me"));
    await Promise.resolve();
    expect(onOpen).not.toHaveBeenCalled();
    expect(screen.getByLabelText("Project folder")).toHaveValue("");
  });

  it("says why the dialog could not be shown", async () => {
    // A Tauri command rejects with its error as a string.
    invoke.mockImplementation(() => Promise.reject("dialog.open not allowed"));
    render(<ProjectBar project={{ cwd: "/p" }} onOpen={() => {}} onCheckSetup={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Choose…" }));

    expect(await screen.findByText("Could not show the folder dialog: dialog.open not allowed")).toHaveClass("problem");
    expectDialogAt("/p");
  });

  it("stops saying why the dialog failed once a project is opened from the bar", async () => {
    invoke.mockImplementation(() => Promise.reject("dialog.open not allowed"));
    const onOpen = vi.fn();
    render(<ProjectBar project={{ cwd: "/p" }} onOpen={onOpen} onCheckSetup={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Choose…" }));
    await screen.findByText("Could not show the folder dialog: dialog.open not allowed");

    fireEvent.click(screen.getByRole("button", { name: "Open" }));

    expect(onOpen).toHaveBeenCalledWith({ cwd: "/p" });
    expect(screen.queryByText(/Could not show the folder dialog/)).not.toBeInTheDocument();
  });

  it("offers neither Open nor Choose… while the dialog is open", async () => {
    let pick: (folder: string | null) => void = () => {};
    invoke.mockImplementation(() => new Promise((resolve) => (pick = resolve)));
    const onOpen = vi.fn();
    render(<ProjectBar project={{ cwd: "/p" }} onOpen={onOpen} onCheckSetup={() => {}} />);
    const choose = screen.getByRole("button", { name: "Choose…" });
    const open = screen.getByRole("button", { name: "Open" });
    fireEvent.click(choose);
    await waitFor(() => expect(invoke).toHaveBeenCalledTimes(1));

    expect(choose).toBeDisabled();
    expect(open).toBeDisabled();
    fireEvent.submit(open);
    expect(onOpen).not.toHaveBeenCalled();

    await act(async () => pick("/q"));
    expect(onOpen).toHaveBeenCalledWith({ cwd: "/q" });
    expect(choose).toBeEnabled();
    expect(open).toBeEnabled();
  });
});
