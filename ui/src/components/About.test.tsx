import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../App";
import { CONNECTION, stubServer } from "../test/fakes";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

beforeEach(() => {
  invoke.mockReset().mockResolvedValue(undefined);
});

afterEach(() => {
  vi.unstubAllEnvs();
});

/** The app with About opened from the top bar's button. */
async function openAbout() {
  stubServer({}, {});
  render(<App connect={() => Promise.resolve(CONNECTION)} />);
  fireEvent.click(await screen.findByRole("button", { name: "About Octoflow AI" }));
  return screen.getByRole("dialog", { name: "about" });
}

describe("About", () => {
  it("opens from the logo's button with the connection's version, the copyright and the license", async () => {
    const about = await openAbout();
    expect(about).toHaveTextContent("Version 0.75.0");
    expect(about).toHaveTextContent("© 2026 David Siegl");
    expect(about).toHaveTextContent("MIT OR Apache-2.0");
  });

  it("asks the shell to open each page by its name", async () => {
    await openAbout();
    const pages = { "Source code": "source", Releases: "releases", "Support on Ko-fi": "ko-fi" };
    for (const [name, link] of Object.entries(pages)) {
      fireEvent.click(screen.getByRole("button", { name }));
      expect(invoke).toHaveBeenLastCalledWith("open_about", { link });
    }
    expect(invoke).toHaveBeenCalledTimes(3);
  });

  it("asks the shell to open the bundled license files in a packaged build", async () => {
    vi.stubEnv("DEV", false);
    await openAbout();
    fireEvent.click(screen.getByRole("button", { name: "Third-party licenses: crates" }));
    expect(invoke).toHaveBeenLastCalledWith("open_about", { link: "crate-licenses" });
    fireEvent.click(screen.getByRole("button", { name: "Third-party licenses: npm packages" }));
    expect(invoke).toHaveBeenLastCalledWith("open_about", { link: "npm-licenses" });
  });

  it("says a development build bundles no license files", async () => {
    vi.stubEnv("DEV", true);
    const about = await openAbout();
    expect(about).toHaveTextContent("Third-party licenses: crates: not bundled in a development build");
    expect(about).toHaveTextContent("Third-party licenses: npm packages: not bundled in a development build");
    expect(screen.queryByRole("button", { name: /Third-party licenses/ })).not.toBeInTheDocument();
  });

  it("says why a link could not be opened", async () => {
    invoke.mockRejectedValue("no browser");
    await openAbout();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Releases" })));
    expect(await screen.findByText("Could not open Releases: no browser")).toBeInTheDocument();
  });

  it("says nothing of a failure that comes after a later click", async () => {
    let fail: (why: string) => void = () => {};
    invoke.mockReturnValueOnce(new Promise((_, reject) => (fail = reject))).mockResolvedValueOnce(undefined);
    await openAbout();
    fireEvent.click(screen.getByRole("button", { name: "Releases" }));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Source code" })));
    await act(async () => fail("no browser"));
    expect(screen.queryByText(/Could not open/)).not.toBeInTheDocument();
  });

  it("closes on Escape and on Close, giving the focus back to the logo's button", async () => {
    await openAbout();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "about" })).not.toBeInTheDocument();
    const badge = screen.getByRole("button", { name: "About Octoflow AI" });
    expect(badge).toHaveFocus();
    fireEvent.click(badge);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog", { name: "about" })).not.toBeInTheDocument();
    expect(badge).toHaveFocus();
  });
});
