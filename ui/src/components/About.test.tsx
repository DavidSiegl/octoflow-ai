import { act, fireEvent, render, screen, within } from "@testing-library/react";
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
  clickBadge(await screen.findByRole("button", { name: "About Octoflow AI" }));
  return screen.getByRole("dialog", { name: "about" });
}

// Clicks the logo's button the way a webview does: jsdom's click alone does not focus it.
function clickBadge(badge: HTMLElement) {
  badge.focus();
  fireEvent.click(badge);
}

describe("About", () => {
  it("opens from the logo's button with the connection's version, the copyright and the license", async () => {
    const about = await openAbout();
    expect(about).toHaveTextContent("Version 0.75.0");
    expect(about).toHaveTextContent("© 2026 David Siegl");
    expect(about).toHaveTextContent("MIT OR Apache-2.0");
  });

  it("asks the shell to open each page by its URL", async () => {
    await openAbout();
    const pages = {
      "Source code": "https://github.com/DavidSiegl/octoflow-ai",
      Releases: "https://github.com/DavidSiegl/octoflow-ai/releases",
      "Support on Ko-fi": "https://ko-fi.com/davidsiegl",
    };
    for (const [name, page] of Object.entries(pages)) {
      fireEvent.click(screen.getByRole("button", { name }));
      expect(invoke).toHaveBeenLastCalledWith("open_outside", { target: { page } });
    }
    expect(invoke).toHaveBeenCalledTimes(3);
  });

  it("asks the shell to open the bundled license files in a packaged build", async () => {
    vi.stubEnv("DEV", false);
    await openAbout();
    fireEvent.click(screen.getByRole("button", { name: "Third-party licenses: crates" }));
    expect(invoke).toHaveBeenLastCalledWith("open_outside", { target: { license: "crate-licenses" } });
    fireEvent.click(screen.getByRole("button", { name: "Third-party licenses: npm packages" }));
    expect(invoke).toHaveBeenLastCalledWith("open_outside", { target: { license: "npm-licenses" } });
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
    const releases = screen.getByRole("button", { name: "Releases" }).closest("li");
    expect(releases).toHaveTextContent("Could not open it: no browser");
  });

  it("says nothing of a failure that comes after a later click", async () => {
    let fail: (why: string) => void = () => {};
    invoke.mockReturnValueOnce(new Promise((_, reject) => (fail = reject))).mockResolvedValueOnce(undefined);
    await openAbout();
    fireEvent.click(screen.getByRole("button", { name: "Releases" }));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Releases" })));
    await act(async () => fail("no browser"));
    expect(screen.queryByText(/Could not open/)).not.toBeInTheDocument();
  });

  it("puts the focus on Close when it opens", async () => {
    const about = await openAbout();
    expect(within(about).getByRole("button", { name: "Close" })).toHaveFocus();
  });

  it("closes on Escape and on Close, giving the focus back to the logo's button once it is no longer inert", async () => {
    await openAbout();
    const badge = screen.getByRole("button", { name: "About Octoflow AI" });
    // jsdom focuses an inert element, which the webviews refuse, so this records where each focus() call lands.
    const focusedInert: boolean[] = [];
    const focus = vi.spyOn(badge, "focus").mockImplementation(function (this: HTMLElement) {
      focusedInert.push(this.closest("[inert]") !== null);
      focus.mockRestore();
      this.focus();
    });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "about" })).not.toBeInTheDocument();
    expect(badge).toHaveFocus();
    expect(focusedInert).toEqual([false]);
    clickBadge(badge);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog", { name: "about" })).not.toBeInTheDocument();
    expect(badge).toHaveFocus();
  });
});
