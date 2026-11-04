import { readFileSync } from "node:fs";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { App } from "./App";
import { CONNECTION, stubServer } from "./test/fakes";

// Read from disk, not imported: Vite's dev server serves nothing outside ui/ by default (server.fs.allow), and
// docs/logo.svg is outside it.
const read = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8");
const original = read("../../docs/logo.svg");
const served = read("../public/logo.svg");
const indexHtml = read("../index.html");

// The logo's accessible name: the <title> docs/logo.svg gives itself.
const title = /<title[^>]*>([^<]+)<\/title>/.exec(original)?.[1];

describe("the logo", () => {
  it("is the copy the page serves of docs/logo.svg, byte for byte", () => {
    expect(served).toBe(original);
  });

  it("names itself in a title", () => {
    expect(title).toBeTruthy();
  });

  it("starts the top bar, beside the product's name and before the project folder", async () => {
    stubServer({}, {});
    render(<App connect={() => Promise.resolve(CONNECTION)} />);
    const logo = await screen.findByRole("img", { name: title });
    expect(logo).toHaveAttribute("src", "/logo.svg");
    const bar = logo.closest(".top-bar");
    expect(bar).toHaveTextContent(/^Octoflow AIProject folder/);
    expect(bar).toContainElement(screen.getByLabelText("Project folder"));
  });

  it("is the page's icon", () => {
    const page = new DOMParser().parseFromString(indexHtml, "text/html");
    expect(page.querySelector('link[rel="icon"]')?.getAttribute("href")).toBe("/logo.svg");
  });
});
