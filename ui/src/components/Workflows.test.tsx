import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { App } from "../App";
import { CONNECTION, DEFAULT_DEFINITION, FakeEventSource, stubServer, workflowAnswers, type Answer, type FakeEdit } from "../test/fakes";

const connect = () => Promise.resolve(CONNECTION);

/** The app with the Workflows dialog open, each entry read; the dialog. */
async function openWorkflows(edit: FakeEdit = {}): Promise<HTMLElement> {
  stubServer({}, {}, {}, {}, {}, edit);
  render(<App connect={connect} />);
  await waitFor(() => expect(FakeEventSource.opened).toHaveLength(1));
  fireEvent.click(screen.getByRole("button", { name: "Workflows…" }));
  const dialog = screen.getByRole("dialog", { name: "workflows" });
  await waitFor(() => expect(within(dialog).queryByText(/Reading/)).not.toBeInTheDocument());
  return dialog;
}

function entry(dialog: HTMLElement, name: string): HTMLElement {
  return within(dialog).getByRole("listitem", { name: `workflow ${name}` });
}

/** The editor, once it has read its workflow and the server has checked it. */
async function editor(): Promise<HTMLElement> {
  const element = await screen.findByRole("dialog", { name: "workflow" });
  await waitFor(() => expect(within(element).queryByText("Reading the workflow…")).not.toBeInTheDocument());
  return element;
}

/** Wait until Save is enabled, the server's check having found no problem. */
async function saveable(element: HTMLElement): Promise<HTMLElement> {
  const save = within(element).getByRole("button", { name: "Save" });
  await waitFor(() => expect(save).toBeEnabled());
  return save;
}

function type(element: HTMLElement, label: string, text: string): void {
  fireEvent.change(within(element).getByLabelText(label), { target: { value: text } });
}

/** Change answers' GET /workflow body for name by change's fields. */
function changeAnswer(answers: Record<string, Answer>, name: string, change: Record<string, unknown>): void {
  const old = answers[name];
  if (!old) {
    throw new Error(`no answer for ${name}`);
  }
  answers[name] = { status: old.status, body: { ...(old.body as object), ...change } };
}

function posts(edit: FakeEdit, path: string) {
  return (edit.posts ?? []).filter((p) => p.path === path).map((p) => p.body);
}

describe("the workflows dialog", () => {
  it("lists every workflow, a built-in one to view and one that does not load with its path, to delete only", async () => {
    const dialog = await openWorkflows();
    const builtIn = entry(dialog, "default");
    expect(builtIn).toHaveTextContent("default (built in)");
    expect(builtIn).toHaveTextContent("Spec, build and review.");
    expect(within(builtIn).getAllByRole("button").map((b) => b.textContent)).toEqual(["View…", "Duplicate…"]);
    const tdd = entry(dialog, "tdd");
    expect(tdd).toHaveTextContent("/cfg/workflows/tdd.toml");
    expect(within(tdd).getAllByRole("button").map((b) => b.textContent)).toEqual(["Edit…", "Duplicate…", "Delete…"]);
    const broken = entry(dialog, "broken");
    expect(broken).toHaveTextContent("/cfg/workflows/broken.toml");
    expect(broken).toHaveTextContent("workflow broken: no steps");
    expect(within(broken).getAllByRole("button").map((b) => b.textContent)).toEqual(["Delete…"]);
  });

  it("creates a workflow from a copy of default, with a new role and step, and marks it in the list", async () => {
    const edit: FakeEdit = {};
    const dialog = await openWorkflows(edit);
    fireEvent.click(within(dialog).getByRole("button", { name: "New…" }));
    const form = await editor();
    type(form, "Name", "mine");
    fireEvent.click(within(form).getByRole("button", { name: "Add role" }));
    type(form, "role 3 key", "test");
    type(form, "role 3 label", "Tester");
    fireEvent.change(within(form).getByLabelText("role 3 agent"), { target: { value: "pi" } });
    fireEvent.click(within(form).getByRole("button", { name: "Add step" }));
    const step = within(form).getByRole("group", { name: "step step3" });
    type(step, "id", "test");
    fireEvent.change(within(form).getByLabelText("role", { selector: "select" }), { target: { value: "test" } });
    type(form, "file", "test.md");
    type(form, "prompt", "Test {build_path}.");
    fireEvent.click(within(form).getByLabelText("human_paced"));
    fireEvent.click(await saveable(form));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "workflow" })).not.toBeInTheDocument());
    expect(posts(edit, "/workflow/save")).toEqual([
      {
        name: "mine",
        base: null,
        definition: {
          ...DEFAULT_DEFINITION,
          roles: { ...DEFAULT_DEFINITION.roles, test: { label: "Tester", agent: "pi" } },
          steps: [...DEFAULT_DEFINITION.steps, { id: "test", role: "test", file: "test.md", prompt: "Test {build_path}.", human_paced: true }],
        },
      },
    ]);
  });

  it("shows the server's problem, and keeps Save disabled while it stands", async () => {
    await openWorkflows({ problem: ({ definition }) => (definition.steps.some((s) => s.id === "review") ? "step review: loop_to x is no step" : null) });
    fireEvent.click(within(screen.getByRole("dialog", { name: "workflows" })).getByRole("button", { name: "New…" }));
    const form = await editor();
    type(form, "Name", "mine");
    await waitFor(() => expect(within(form).getByText("step review: loop_to x is no step")).toBeInTheDocument());
    expect(within(form).getByRole("button", { name: "Save" })).toBeDisabled();
    fireEvent.click(within(form).getByRole("button", { name: "review" }));
    fireEvent.click(within(form).getByRole("button", { name: "Remove step" }));
    await saveable(form);
    expect(within(form).queryByText(/loop_to x/)).not.toBeInTheDocument();
  });

  it("names two roles under one key without asking the server", async () => {
    const edit: FakeEdit = {};
    await openWorkflows(edit);
    fireEvent.click(within(screen.getByRole("dialog", { name: "workflows" })).getByRole("button", { name: "New…" }));
    const form = await editor();
    await saveable(form);
    const checks = posts(edit, "/workflow/check").length;
    type(form, "role 2 key", "build");
    expect(within(form).getByText("two roles have the key build")).toBeInTheDocument();
    expect(within(form).getByRole("button", { name: "Save" })).toBeDisabled();
    await new Promise((resolve) => setTimeout(resolve, 400));
    expect(posts(edit, "/workflow/check")).toHaveLength(checks);
  });

  it("opens a built-in workflow read-only, with Duplicate… in place of Save, which makes it a new one", async () => {
    const dialog = await openWorkflows();
    fireEvent.click(within(entry(dialog, "default")).getByRole("button", { name: "View…" }));
    const form = await editor();
    expect(within(form).getByRole("heading", { level: 2 })).toHaveTextContent("default (built in, read-only)");
    expect(within(form).getByLabelText("Description")).toBeDisabled();
    expect(within(form).queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
    fireEvent.click(within(form).getByRole("button", { name: "Duplicate…" }));
    expect(within(form).getByLabelText("Name")).toBeEnabled();
    expect(within(form).getByLabelText("Description")).toBeEnabled();
    expect(within(form).getByRole("button", { name: "Save" })).toBeInTheDocument();
  });

  it("shows a failed write, and stays open with the edits", async () => {
    await openWorkflows({ save: { status: 500, body: { error: "cannot write /cfg/workflows/mine.toml: Read-only file system" } } });
    fireEvent.click(within(screen.getByRole("dialog", { name: "workflows" })).getByRole("button", { name: "New…" }));
    const form = await editor();
    type(form, "Name", "mine");
    fireEvent.click(await saveable(form));
    expect(await within(form).findByText(/Read-only file system/)).toBeInTheDocument();
    expect(within(form).getByLabelText("Name")).toHaveValue("mine");
    expect(within(form).queryByRole("button", { name: "Reload" })).not.toBeInTheDocument();
  });

  it("asks before rewriting a hand-written file, and Cancel sends nothing", async () => {
    const workflows = workflowAnswers();
    changeAnswer(workflows, "tdd", { hand_written: true });
    const edit: FakeEdit = { workflows };
    const dialog = await openWorkflows(edit);
    fireEvent.click(within(entry(dialog, "tdd")).getByRole("button", { name: "Edit…" }));
    const form = await editor();
    type(form, "Description", "Changed.");
    fireEvent.click(await saveable(form));
    const ask = screen.getByRole("dialog", { name: "rewrite hand-written file" });
    expect(ask).toHaveTextContent("/cfg/workflows/tdd.toml was written by hand; saving rewrites it without its comments");
    fireEvent.click(within(ask).getByRole("button", { name: "Cancel" }));
    expect(posts(edit, "/workflow/save")).toEqual([]);
    fireEvent.click(within(form).getByRole("button", { name: "Save" }));
    fireEvent.click(within(screen.getByRole("dialog", { name: "rewrite hand-written file" })).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(posts(edit, "/workflow/save")).toHaveLength(1));
    expect(posts(edit, "/workflow/save")[0]).toMatchObject({ name: "tdd", base: "tdd's text" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "workflow" })).not.toBeInTheDocument());
    await waitFor(() => expect(entry(dialog, "tdd")).toHaveAttribute("aria-current", "true"));
  });

  it("shows a file changed since it was opened, and Reload reads it again after a confirmation", async () => {
    const workflows = workflowAnswers();
    const edit: FakeEdit = { workflows, save: { status: 409, body: { error: "/cfg/workflows/tdd.toml changed since it was opened" } } };
    const dialog = await openWorkflows(edit);
    fireEvent.click(within(entry(dialog, "tdd")).getByRole("button", { name: "Edit…" }));
    const form = await editor();
    type(form, "Description", "Changed.");
    fireEvent.click(await saveable(form));
    expect(await within(form).findByText("/cfg/workflows/tdd.toml changed since it was opened")).toBeInTheDocument();
    changeAnswer(workflows, "tdd", { text: "new text" });
    fireEvent.click(within(form).getByRole("button", { name: "Reload" }));
    const ask = screen.getByRole("dialog", { name: "reload workflow" });
    expect(ask).toHaveTextContent("Reading /cfg/workflows/tdd.toml again discards the changes made here.");
    fireEvent.click(within(ask).getByRole("button", { name: "Reload" }));
    await waitFor(() => expect(within(form).getByLabelText("Description")).toHaveValue("Tests first, then the code."));
    expect(within(form).queryByText(/changed since/)).not.toBeInTheDocument();
    edit.save = undefined;
    type(form, "Description", "Changed again.");
    fireEvent.click(await saveable(form));
    await waitFor(() => expect(posts(edit, "/workflow/save").at(-1)).toMatchObject({ base: "new text" }));
  });

  it("deletes a workflow's file after a confirmation naming it, and reads the list again", async () => {
    const edit: FakeEdit = {};
    const dialog = await openWorkflows(edit);
    fireEvent.click(within(entry(dialog, "broken")).getByRole("button", { name: "Delete…" }));
    const ask = screen.getByRole("dialog", { name: "delete workflow" });
    expect(ask).toHaveTextContent("This removes /cfg/workflows/broken.toml.");
    fireEvent.click(within(ask).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(posts(edit, "/workflow/delete")).toEqual([{ name: "broken", base: "broken's text" }]));
    expect(screen.queryByRole("dialog", { name: "delete workflow" })).not.toBeInTheDocument();
  });

  it("deletes nothing when the confirmation is cancelled", async () => {
    const edit: FakeEdit = {};
    const dialog = await openWorkflows(edit);
    fireEvent.click(within(entry(dialog, "tdd")).getByRole("button", { name: "Delete…" }));
    fireEvent.click(within(screen.getByRole("dialog", { name: "delete workflow" })).getByRole("button", { name: "Cancel" }));
    expect(posts(edit, "/workflow/delete")).toEqual([]);
  });

  it("asks Save, Discard or Cancel when closing with unsaved changes", async () => {
    const dialog = await openWorkflows();
    fireEvent.click(within(entry(dialog, "tdd")).getByRole("button", { name: "Edit…" }));
    const form = await editor();
    type(form, "Description", "Changed.");
    fireEvent.click(within(form).getByRole("button", { name: "Close" }));
    const ask = screen.getByRole("dialog", { name: "unsaved changes" });
    expect(within(ask).getAllByRole("button").map((b) => b.textContent)).toEqual(["Cancel", "Discard", "Save"]);
    fireEvent.click(within(ask).getByRole("button", { name: "Cancel" }));
    expect(within(form).getByLabelText("Description")).toHaveValue("Changed.");
    fireEvent.click(within(form).getByRole("button", { name: "Close" }));
    fireEvent.click(within(screen.getByRole("dialog", { name: "unsaved changes" })).getByRole("button", { name: "Discard" }));
    expect(screen.queryByRole("dialog", { name: "workflow" })).not.toBeInTheDocument();
  });

  it("closes without asking when nothing changed", async () => {
    const dialog = await openWorkflows();
    fireEvent.click(within(entry(dialog, "tdd")).getByRole("button", { name: "Edit…" }));
    const form = await editor();
    fireEvent.click(within(form).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog", { name: "workflow" })).not.toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "unsaved changes" })).not.toBeInTheDocument();
  });
});
