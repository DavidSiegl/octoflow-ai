import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { STEP_FLAG_HELP, STEP_FLAGS, type WorkflowDefinition } from "../api";
import { App } from "../App";
import { CONNECTION, FakeWindow, DEFAULT_DEFINITION, FakeEventSource, OPTIONS, stubServer, workflowAnswers, type Answer, type FakeEdit, type FakeStart } from "../test/fakes";

const connect = () => Promise.resolve(CONNECTION);

/** The app with the Workflows dialog open, each entry read; the dialog. */
async function openWorkflows(edit: FakeEdit = {}): Promise<HTMLElement> {
  stubServer({}, {}, {}, {}, {}, edit);
  render(<App connect={connect} appWindow={new FakeWindow()} />);
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

/** Pick the option of select that shows text. */
function choose(select: HTMLElement, text: string): void {
  const option = within(select).getByRole("option", { name: text }) as HTMLOptionElement;
  fireEvent.change(select, { target: { value: option.value } });
}

/** The editor on a copy of default, named mine, once its check passed; the editor. */
async function newMine(edit: FakeEdit = {}): Promise<HTMLElement> {
  await openWorkflows(edit);
  fireEvent.click(within(screen.getByRole("dialog", { name: "workflows" })).getByRole("button", { name: "New…" }));
  const form = await editor();
  type(form, "Name", "mine");
  await saveable(form);
  return form;
}

/** The definition the last check or save sent. */
function lastSent(edit: FakeEdit): WorkflowDefinition {
  return edit.posts?.at(-1)?.body.definition as WorkflowDefinition;
}

/** The definition of workflow name in answers, a GET /workflow answer that loads. */
function definitionIn(answers: Record<string, Answer>, name: string): WorkflowDefinition {
  return (answers[name]?.body as { definition: WorkflowDefinition }).definition;
}

function posts(edit: FakeEdit, path: string) {
  return (edit.posts ?? []).filter((p) => p.path === path).map((p) => p.body);
}

// The editor's form is large, and each query by role computes the accessible name of every element in it, which in
// jsdom takes the longest of these tests about 2 s, and past the default 5 s when the machine is busy.
const EDITOR_TIMEOUT_MS = 15_000;

describe("the workflows dialog", { timeout: EDITOR_TIMEOUT_MS }, () => {
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
    choose(within(form).getByLabelText("role", { selector: "select" }), "test");
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

  it("keeps a loop on its step while the step's id is retyped, through empty and through another step's id", async () => {
    const edit: FakeEdit = {};
    const form = await newMine(edit);
    fireEvent.click(within(form).getByRole("button", { name: "build" }));
    for (const id of ["", "c", "review", "built"]) {
      type(form, "id", id);
    }
    await saveable(form);
    expect(lastSent(edit).steps.map((s) => [s.id, s.loop_to])).toEqual([["built", undefined], ["review", "built"]]);
  });

  it("keeps each role's steps while a role key is retyped through another's", async () => {
    const edit: FakeEdit = {};
    const form = await newMine(edit);
    type(form, "role 2 key", "build");
    expect(within(form).getByText("two roles have the key build")).toBeInTheDocument();
    type(form, "role 2 key", "check");
    await saveable(form);
    expect(lastSent(edit).steps.map((s) => [s.id, s.role])).toEqual([["build", "build"], ["review", "check"]]);
  });

  it("names a loop back to a removed step, shown as removed, until another target is picked", async () => {
    const form = await newMine();
    fireEvent.click(within(form).getByRole("button", { name: "build" }));
    fireEvent.click(within(form).getByRole("button", { name: "Remove step" }));
    expect(within(form).getByText(/step review loops back to a step that was removed/)).toBeInTheDocument();
    const loop = within(form).getByLabelText("loop_to");
    expect((loop as HTMLSelectElement).selectedOptions[0]?.textContent).toBe("(removed)");
    expect(within(form).getByRole("button", { name: "Save" })).toBeDisabled();
    choose(loop, "none");
    await saveable(form);
    expect(within(loop).getAllByRole("option").map((o) => o.textContent)).toEqual(["none"]);
  });

  it("names a step whose role was removed, until another role is picked", async () => {
    const form = await newMine();
    const removes = within(form).getAllByRole("button", { name: "Remove" });
    fireEvent.click(removes[1] as HTMLElement);
    expect(within(form).getByText(/has no role: pick one/)).toBeInTheDocument();
    fireEvent.click(within(form).getByRole("button", { name: "review" }));
    choose(within(form).getByLabelText("role", { selector: "select" }), "build");
    await saveable(form);
  });

  it("gives an added step an id no step has", async () => {
    const form = await newMine();
    fireEvent.click(within(form).getByRole("button", { name: "Add step" }));
    fireEvent.click(within(form).getByRole("button", { name: "Add step" }));
    fireEvent.click(within(form).getByRole("button", { name: "step3" }));
    fireEvent.click(within(form).getByRole("button", { name: "Remove step" }));
    fireEvent.click(within(form).getByRole("button", { name: "Add step" }));
    expect(within(form).getAllByRole("button", { name: /^step\d$/ }).map((b) => b.textContent)).toEqual(["step4", "step5"]);
  });

  it("shows the server's problem with the name, a built-in one", async () => {
    await openWorkflows({ problem: ({ name }) => (name === "default" ? "workflow default is built in, so no file may take its name" : null) });
    fireEvent.click(within(screen.getByRole("dialog", { name: "workflows" })).getByRole("button", { name: "New…" }));
    const form = await editor();
    type(form, "Name", "default");
    expect(await within(form).findByText("workflow default is built in, so no file may take its name")).toBeInTheDocument();
    expect(within(form).getByRole("button", { name: "Save" })).toBeDisabled();
  });

  it("shows a new workflow's name that is taken, with no Reload", async () => {
    const form = await newMine({ save: { status: 409, body: { error: "workflow mine exists already" } } });
    fireEvent.click(within(form).getByRole("button", { name: "Save" }));
    expect(await within(form).findByText("workflow mine exists already")).toBeInTheDocument();
    expect(within(form).queryByRole("button", { name: "Reload" })).not.toBeInTheDocument();
  });

  it("shows an error answer that is not JSON by its status", async () => {
    const dialog = await openWorkflows({ workflows: { ...workflowAnswers(), tdd: { status: 502, body: undefined } } });
    expect(entry(dialog, "tdd")).toHaveTextContent("HTTP 502");
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

  it("shows no input for max_files, max_lines or proof_command, and keeps a file's values of them through an edit and a copy", async () => {
    const workflows = workflowAnswers();
    const settings = { max_files: 3, max_lines: 120, proof_command: "make proof {proofs}" };
    const tdd = definitionIn(workflows, "tdd");
    // A verdict step, which a proof_command needs.
    const steps = tdd.steps.map((s) => (s.id === "build" ? { ...s, loop_to: "test" } : s));
    changeAnswer(workflows, "tdd", { definition: { ...tdd, ...settings, steps } });
    const edit: FakeEdit = { workflows };
    const dialog = await openWorkflows(edit);
    fireEvent.click(within(entry(dialog, "tdd")).getByRole("button", { name: "Edit…" }));
    const form = await editor();
    for (const label of ["max_files", "max_lines", "proof_command"]) {
      expect(within(form).queryByLabelText(label)).not.toBeInTheDocument();
    }
    type(form, "prompt", "Write the tests first.");
    fireEvent.click(await saveable(form));
    await waitFor(() => expect(posts(edit, "/workflow/save")).toHaveLength(1));
    expect(posts(edit, "/workflow/save")[0]?.definition).toMatchObject({ ...settings, steps: [{ prompt: "Write the tests first." }, {}] });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "workflow" })).not.toBeInTheDocument());
    // The list is read again after the save.
    fireEvent.click(await within(await within(dialog).findByRole("listitem", { name: "workflow tdd" })).findByRole("button", { name: "Duplicate…" }));
    const copy = await editor();
    type(copy, "Name", "mine");
    fireEvent.click(await saveable(copy));
    await waitFor(() => expect(posts(edit, "/workflow/save")).toHaveLength(2));
    expect(posts(edit, "/workflow/save")[1]).toMatchObject({ name: "mine", definition: settings });
  });

  it("leaves proof_command out while no step is a verdict step, and writes it again once one is", async () => {
    const workflows = workflowAnswers();
    changeAnswer(workflows, "default", { definition: { ...DEFAULT_DEFINITION, proof_command: "pytest {proofs}" } });
    const edit: FakeEdit = { workflows };
    const form = await newMine(edit);
    expect(lastSent(edit).proof_command).toBe("pytest {proofs}");
    fireEvent.click(within(form).getByRole("button", { name: "review" }));
    choose(within(form).getByLabelText("loop_to"), "none");
    await waitFor(() => expect(lastSent(edit).steps.at(-1)?.loop_to).toBeUndefined());
    await saveable(form);
    expect(lastSent(edit)).not.toHaveProperty("proof_command");
    choose(within(form).getByLabelText("loop_to"), "build");
    await waitFor(() => expect(lastSent(edit).proof_command).toBe("pytest {proofs}"));
  });

  it("keeps a built-in workflow's max_files, max_lines and proof_command in a copy made from its view", async () => {
    const workflows = workflowAnswers();
    const settings = { max_files: 3, max_lines: 120, proof_command: "pytest {proofs}" };
    changeAnswer(workflows, "default", { definition: { ...DEFAULT_DEFINITION, ...settings } });
    const edit: FakeEdit = { workflows };
    const dialog = await openWorkflows(edit);
    fireEvent.click(within(entry(dialog, "default")).getByRole("button", { name: "View…" }));
    const form = await editor();
    fireEvent.click(within(form).getByRole("button", { name: "Duplicate…" }));
    type(form, "Name", "mine");
    fireEvent.click(await saveable(form));
    await waitFor(() => expect(posts(edit, "/workflow/save")).toHaveLength(1));
    expect(posts(edit, "/workflow/save")[0]).toMatchObject({ name: "mine", definition: settings });
  });

  it("writes no max_files, max_lines or proof_command for a workflow that sets none", async () => {
    const edit: FakeEdit = {};
    const dialog = await openWorkflows(edit);
    fireEvent.click(within(entry(dialog, "tdd")).getByRole("button", { name: "Edit…" }));
    const form = await editor();
    type(form, "prompt", "Write the tests first.");
    fireEvent.click(await saveable(form));
    await waitFor(() => expect(posts(edit, "/workflow/save")).toHaveLength(1));
    const saved = posts(edit, "/workflow/save")[0]?.definition;
    expect(saved).toBeDefined();
    for (const key of ["max_files", "max_lines", "proof_command"]) {
      expect(saved).not.toHaveProperty(key);
    }
  });

  it("explains each step flag on hover and as its checkbox's description, read-only and editable alike", async () => {
    const dialog = await openWorkflows();
    fireEvent.click(within(entry(dialog, "default")).getByRole("button", { name: "View…" }));
    const form = await editor();
    const explained = () => {
      for (const flag of STEP_FLAGS) {
        const box = within(form).getByRole("checkbox", { name: flag, description: STEP_FLAG_HELP[flag] });
        expect(box.closest("label")).toHaveAttribute("title", STEP_FLAG_HELP[flag]);
      }
    };
    explained();
    fireEvent.click(within(form).getByRole("button", { name: "Duplicate…" }));
    expect(within(form).getByRole("checkbox", { name: "edits" })).toBeEnabled();
    explained();
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

  it("shows a delete under way as Deleting…, with nothing to click on its entry or Close until it is answered", async () => {
    let release = () => {};
    const edit: FakeEdit = { removeHeld: new Promise((resolve) => (release = resolve)) };
    const dialog = await openWorkflows(edit);
    fireEvent.click(within(entry(dialog, "tdd")).getByRole("button", { name: "Delete…" }));
    fireEvent.click(within(screen.getByRole("dialog", { name: "delete workflow" })).getByRole("button", { name: "Delete" }));

    expect(within(entry(dialog, "tdd")).getByText("Deleting…")).toBeInTheDocument();
    expect(within(entry(dialog, "tdd")).queryByRole("button")).not.toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Close" })).toBeDisabled();
    await act(async () => release());
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Close" })).toBeEnabled());
    expect(posts(edit, "/workflow/delete")).toHaveLength(1);
  });

  it("clears a failed delete's reason once a workflow is saved", async () => {
    const edit: FakeEdit = { remove: { status: 409, body: { error: "tdd.toml changed since it was read" } } };
    const dialog = await openWorkflows(edit);
    fireEvent.click(within(entry(dialog, "tdd")).getByRole("button", { name: "Delete…" }));
    fireEvent.click(within(screen.getByRole("dialog", { name: "delete workflow" })).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByText("tdd.toml changed since it was read")).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("button", { name: "New…" }));
    const form = await editor();
    type(form, "Name", "mine");
    fireEvent.click(await saveable(form));

    await waitFor(() => expect(within(dialog).queryByText("tdd.toml changed since it was read")).not.toBeInTheDocument());
  });

  it("clears a failed delete's reason once a later delete is confirmed", async () => {
    const edit: FakeEdit = { remove: { status: 409, body: { error: "tdd.toml changed since it was read" } } };
    const dialog = await openWorkflows(edit);
    fireEvent.click(within(entry(dialog, "tdd")).getByRole("button", { name: "Delete…" }));
    fireEvent.click(within(screen.getByRole("dialog", { name: "delete workflow" })).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByText("tdd.toml changed since it was read")).toBeInTheDocument();

    edit.remove = undefined;
    fireEvent.click(within(entry(dialog, "broken")).getByRole("button", { name: "Delete…" }));
    fireEvent.click(within(screen.getByRole("dialog", { name: "delete workflow" })).getByRole("button", { name: "Delete" }));
    expect(within(dialog).queryByText("tdd.toml changed since it was read")).not.toBeInTheDocument();
    await waitFor(() => expect(posts(edit, "/workflow/delete")).toHaveLength(2));
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

describe("the run form's Edit… and New…", { timeout: EDITOR_TIMEOUT_MS }, () => {
  /** The app with the run form open on `default`, the workflows read. */
  async function openForm(start: FakeStart, edit: FakeEdit = {}): Promise<HTMLElement> {
    stubServer({}, {}, {}, start, {}, edit);
    render(<App connect={connect} appWindow={new FakeWindow()} />);
    await waitFor(() => expect(FakeEventSource.opened).toHaveLength(1));
    fireEvent.click(screen.getByRole("button", { name: "New run…" }));
    const form = await screen.findByRole("dialog", { name: "new run" });
    await waitFor(() => expect(within(form).getByLabelText("Workflow")).toHaveValue("default"));
    return form;
  }

  it("edits the workflow picked, and returns with it picked and its roles' rows rebuilt", async () => {
    const start: FakeStart = { options: structuredClone(OPTIONS) };
    const edit: FakeEdit = {};
    const form = await openForm(start, edit);
    fireEvent.change(within(form).getByLabelText("Workflow"), { target: { value: "tdd" } });
    fireEvent.change(within(form).getByLabelText("Builder model"), { target: { value: "g2" } });
    fireEvent.click(within(form).getByRole("button", { name: "Edit…" }));
    const editor = await screen.findByRole("dialog", { name: "workflow" });
    await waitFor(() => expect(within(editor).getByLabelText("Description")).toHaveValue("Tests first, then the code."));
    fireEvent.change(within(editor).getByLabelText("role 1 label"), { target: { value: "Tester" } });
    // What /workflows answers once the save has renamed the label.
    const tdd = start.options?.workflows[1];
    if (tdd && "roles" in tdd && tdd.roles[0]) {
      tdd.roles[0].label = "Tester";
    }
    const save = within(editor).getByRole("button", { name: "Save" });
    await waitFor(() => expect(save).toBeEnabled());
    fireEvent.click(save);
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "workflow" })).not.toBeInTheDocument());
    await waitFor(() => expect(within(form).getByLabelText("Tester agent")).toBeInTheDocument());
    expect(within(form).getByLabelText("Workflow")).toHaveValue("tdd");
    expect(within(form).getByLabelText("Builder model")).toHaveValue("g2");
    expect(posts(edit, "/workflow/save")[0]).toMatchObject({ name: "tdd", base: "tdd's text" });
  });

  it("creates a workflow from a copy of default and picks it, keeping the rows of the roles it shares", async () => {
    const start: FakeStart = { options: structuredClone(OPTIONS) };
    const form = await openForm(start);
    fireEvent.change(within(form).getByLabelText("Reviewer agent"), { target: { value: "pi" } });
    fireEvent.click(within(form).getByRole("button", { name: "New…" }));
    const editor = await screen.findByRole("dialog", { name: "workflow" });
    await waitFor(() => expect(within(editor).getByLabelText("Name")).toBeInTheDocument());
    fireEvent.change(within(editor).getByLabelText("Name"), { target: { value: "mine" } });
    start.options?.workflows.push({
      name: "mine",
      description: "",
      roles: [
        { role: "review", label: "Reviewer", agent: "codex", model: "o3" },
        { role: "check", label: "Checker", agent: "claude", model: null },
      ],
    });
    const save = within(editor).getByRole("button", { name: "Save" });
    await waitFor(() => expect(save).toBeEnabled());
    fireEvent.click(save);
    await waitFor(() => expect(within(form).getByLabelText("Workflow")).toHaveValue("mine"));
    expect(within(form).getByLabelText("Reviewer agent")).toHaveValue("pi");
    expect(within(form).getByLabelText("Checker agent")).toHaveValue("");
    expect(within(form).queryByLabelText("Builder agent")).not.toBeInTheDocument();
  });

  it("disables Edit… and New… while the run starts", async () => {
    let release = () => {};
    const start: FakeStart = { held: new Promise((resolve) => (release = resolve)) };
    const form = await openForm(start);
    expect(within(form).getByRole("button", { name: "Edit…" })).toBeEnabled();
    fireEvent.change(within(form).getByLabelText("Task"), { target: { value: "a task" } });
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    await waitFor(() => expect(within(form).getByRole("button", { name: "Edit…" })).toBeDisabled());
    expect(within(form).getByRole("button", { name: "New…" })).toBeDisabled();
    release();
  });

  /** Edit… on the workflow picked in form, a change of its description, and Save, once enabled. */
  async function editAndSave(form: HTMLElement): Promise<void> {
    fireEvent.click(within(form).getByRole("button", { name: "Edit…" }));
    const editor = await screen.findByRole("dialog", { name: "workflow" });
    await waitFor(() => expect(within(editor).getByLabelText("Description")).toBeEnabled());
    fireEvent.change(within(editor).getByLabelText("Description"), { target: { value: "Changed." } });
    const save = within(editor).getByRole("button", { name: "Save" });
    await waitFor(() => expect(save).toBeEnabled());
    fireEvent.click(save);
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "workflow" })).not.toBeInTheDocument());
  }

  it("keeps the form, Start and Cancel off while the editor is open", async () => {
    const form = await openForm({});
    fireEvent.change(within(form).getByLabelText("Task"), { target: { value: "a task" } });
    fireEvent.click(within(form).getByRole("button", { name: "Edit…" }));
    await screen.findByRole("dialog", { name: "workflow" });
    expect(within(form).getByLabelText("Task")).toBeDisabled();
    expect(within(form).getByRole("button", { name: "Start" })).toBeDisabled();
    expect(within(form).getByRole("button", { name: "Cancel" })).toBeDisabled();
  });

  it("cannot start until the workflows are read again after a save", async () => {
    const start: FakeStart = {};
    const form = await openForm(start);
    fireEvent.change(within(form).getByLabelText("Task"), { target: { value: "a task" } });
    fireEvent.change(within(form).getByLabelText("Workflow"), { target: { value: "tdd" } });
    // Each /workflows read waits at the server until its answer, kept here, is called. The form issues the read after
    // the save from an effect, which React may run after editAndSave returns, so the test waits for the read to arrive
    // rather than answering at once: an answer before it would answer nothing and leave the read waiting for good.
    const answers: (() => void)[] = [];
    const fetch = globalThis.fetch;
    vi.stubGlobal("fetch", async (url: string, init?: RequestInit) => {
      if (new URL(url).pathname === "/workflows") {
        await new Promise<void>((answer) => answers.push(answer));
      }
      return fetch(url, init);
    });
    await editAndSave(form);
    expect(within(form).getByText("Reading the workflows…")).toBeInTheDocument();
    expect(within(form).getByRole("button", { name: "Start" })).toBeDisabled();
    await waitFor(() => expect(answers, "one read of /workflows after the save").toHaveLength(1), { timeout: 5000 });
    expect(within(form).getByRole("button", { name: "Start" })).toBeDisabled();
    answers.forEach((answer) => answer());
    await waitFor(() => expect(within(form).getByRole("button", { name: "Start" })).toBeEnabled());
    expect(within(form).getByLabelText("Workflow")).toHaveValue("tdd");
  });

  it("sends only the rows of the workflow it runs when the read after a save fails", async () => {
    const start: FakeStart = {};
    const form = await openForm(start);
    fireEvent.change(within(form).getByLabelText("Task"), { target: { value: "a task" } });
    fireEvent.change(within(form).getByLabelText("Workflow"), { target: { value: "tdd" } });
    fireEvent.change(within(form).getByLabelText("Test Writer agent"), { target: { value: "pi" } });
    start.optionsError = "cannot list workflows";
    await editAndSave(form);
    await waitFor(() => expect(within(form).getByText(/cannot list workflows/)).toBeInTheDocument());
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    await waitFor(() => expect(start.requests).toHaveLength(1));
    expect(start.requests?.[0]?.body).toMatchObject({ workflow: "default" });
    expect(start.requests?.[0]?.body.role_agents).toBeUndefined();
  });

  it("falls back to default when the workflow saved does not load when read again", async () => {
    const start: FakeStart = { options: structuredClone(OPTIONS) };
    const form = await openForm(start);
    fireEvent.change(within(form).getByLabelText("Workflow"), { target: { value: "tdd" } });
    start.options = { ...OPTIONS, workflows: [...OPTIONS.workflows.filter((w) => w.name !== "tdd"), { name: "tdd", error: "tdd: no steps" }] };
    await editAndSave(form);
    await waitFor(() => expect(within(form).getByLabelText("Workflow")).toHaveValue("default"));
  });

  it("returns to the form unchanged when the editor closes without saving", async () => {
    const edit: FakeEdit = {};
    const form = await openForm({}, edit);
    fireEvent.click(within(form).getByRole("button", { name: "Edit…" }));
    const editor = await screen.findByRole("dialog", { name: "workflow" });
    fireEvent.click(await within(editor).findByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog", { name: "workflow" })).not.toBeInTheDocument();
    expect(within(form).getByLabelText("Workflow")).toHaveValue("default");
    expect(posts(edit, "/workflow/save")).toEqual([]);
  });
});
