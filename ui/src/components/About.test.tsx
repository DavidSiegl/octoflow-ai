import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../App";
import { CONNECTION, stubServer } from "../test/fakes";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/path", () => ({
  resolveResource: (file: string) => Promise.resolve(`/opt/octoflow/${file}`),
}));
const LICENSES = "/opt/octoflow/THIRD-PARTY-LICENSES";

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

  it("opens each page in the system's browser", async () => {
    await openAbout();
    const pages = {
      "Source code": "https://github.com/DavidSiegl/octoflow-ai",
      Releases: "https://github.com/DavidSiegl/octoflow-ai/releases",
      "Support on Ko-fi": "https://ko-fi.com/davidsiegl",
    };
    for (const [name, url] of Object.entries(pages)) {
      fireEvent.click(screen.getByRole("link", { name }));
      expect(invoke).toHaveBeenLastCalledWith("plugin:opener|open_url", { url });
    }
    expect(invoke).toHaveBeenCalledTimes(3);
  });

  it("opens the bundled license files in a packaged build", async () => {
    vi.stubEnv("DEV", false);
    await openAbout();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Third-party licenses: crates" })));
    expect(invoke).toHaveBeenLastCalledWith("plugin:opener|open_path", { path: `${LICENSES}.html` });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Third-party licenses: npm packages" })));
    expect(invoke).toHaveBeenLastCalledWith("plugin:opener|open_path", { path: `${LICENSES}-NPM.html` });
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
    await act(async () => fireEvent.click(screen.getByRole("link", { name: "Releases" })));
    const reason = "Could not open https://github.com/DavidSiegl/octoflow-ai/releases: no browser";
    expect(await screen.findByText(reason)).toBeInTheDocument();
  });

  it("closes on Escape and on Close", async () => {
    await openAbout();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "about" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "About Octoflow AI" }));
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog", { name: "about" })).not.toBeInTheDocument();
  });
});
