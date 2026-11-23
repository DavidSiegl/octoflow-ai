import { act, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Api } from "../api";
import { HandoffFile } from "./HandoffFile";

const PROJECT = { cwd: "/home/me/proj", machine: "" };
const RUN = "20261122-120000-c0ffee";

/** An Api whose fileText answers only when the test settles it, so a test can look at the page while a read is
 * pending. settle(n, text) answers the nth read (from 0) with text, or rejects it with an Error. */
function pendingReads() {
  const answers: ((text: string | Error) => void)[] = [];
  const fileText = () =>
    new Promise<string>((resolve, reject) => {
      answers.push((text) => (text instanceof Error ? reject(text) : resolve(text)));
    });
  const settle = (n: number, text: string | Error) =>
    act(async () => {
      const answer = answers[n];
      if (!answer) {
        throw new Error(`read ${n} never started; ${answers.length} did`);
      }
      answer(text);
    });
  return { api: { fileText } as unknown as Api, reads: answers, settle };
}

describe("the handoff file on show", () => {
  it("keeps the text it shows while a newer version of the file is read, then shows the newer", async () => {
    const { api, reads, settle } = pendingReads();
    const { rerender } = render(<HandoffFile api={api} project={PROJECT} run={RUN} file={{ name: "spec.md", mtime: 1 }} />);
    await settle(0, "# First draft");
    expect(screen.getByRole("heading", { name: "First draft" })).toBeTruthy();

    rerender(<HandoffFile api={api} project={PROJECT} run={RUN} file={{ name: "spec.md", mtime: 2 }} />);
    expect(reads).toHaveLength(2);
    expect(screen.queryByText(/Loading/)).toBeNull();
    expect(screen.getByRole("heading", { name: "First draft" })).toBeTruthy();

    await settle(1, "# Second draft");
    expect(screen.getByRole("heading", { name: "Second draft" })).toBeTruthy();
  });

  it("shows another file chosen as loading until it arrives, never the last file's text", async () => {
    const { api, settle } = pendingReads();
    const { rerender } = render(<HandoffFile api={api} project={PROJECT} run={RUN} file={{ name: "spec.md", mtime: 1 }} />);
    await settle(0, "# The spec");

    rerender(<HandoffFile api={api} project={PROJECT} run={RUN} file={{ name: "plan.md", mtime: 1 }} />);
    expect(screen.getByText("Loading plan.md…")).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "The spec" })).toBeNull();

    await settle(1, "# The plan");
    expect(screen.getByRole("heading", { name: "The plan" })).toBeTruthy();
  });

  it("shows the error of a failed read of a newer version", async () => {
    const { api, settle } = pendingReads();
    const { rerender } = render(<HandoffFile api={api} project={PROJECT} run={RUN} file={{ name: "spec.md", mtime: 1 }} />);
    await settle(0, "# First draft");

    rerender(<HandoffFile api={api} project={PROJECT} run={RUN} file={{ name: "spec.md", mtime: 2 }} />);
    await settle(1, new Error("spec.md: no such file"));
    expect(screen.getByText("spec.md: no such file")).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "First draft" })).toBeNull();
  });
});
