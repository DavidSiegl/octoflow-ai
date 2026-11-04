import { readFileSync } from "node:fs";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { App } from "./App";
import { CONNECTION, stubServer } from "./test/fakes";

// Read from disk, not imported: Vite refuses files outside ui/, and docs/logo.svg is one.
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

  it("heads the window, beside the product's name", () => {
    stubServer({}, {});
    render(<App connect={() => Promise.resolve(CONNECTION)} />);
    const logo = screen.getByRole("img", { name: title });
    expect(logo).toHaveAttribute("src", "/logo.svg");
    expect(screen.getByRole("banner")).toHaveTextContent("Octoflow AI");
  });

  it("is the page's icon", () => {
    const page = new DOMParser().parseFromString(indexHtml, "text/html");
    expect(page.querySelector('link[rel="icon"]')?.getAttribute("href")).toBe("/logo.svg");
  });
});
