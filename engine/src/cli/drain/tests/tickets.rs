//! drain over a spec's tickets, DIR/<slug>/issues/<NN>-<name>.md (queue::Kind), on the world of the tests beside it.

use super::*;

const PR_C: &str = "https://github.com/o/r/pull/3";

/// A ready ticket without further header lines.
fn ready_ticket(title: &str) -> String {
    spec_text(title, Some(SPEC_READY), &[])
}

/// The status drain writes for a ticket whose pull request number n merged.
fn resolved(n: u32) -> Option<String> {
    Some(format!("{TICKET_RESOLVED} (PR #{n} merged)"))
}

#[test]
fn a_specs_tickets_run_in_number_order_each_as_its_own_run_and_pull_request() {
    let d = Drain::new();
    d.spec("s", &ready("Spec"));
    d.ticket("s", "10-tenth", &ready_ticket("Tenth"));
    d.ticket("s", "02-second", &ready_ticket("Second"));
    d.ticket("s", "01-first", &ready_ticket("First"));
    d.ticket("s", "notes", &ready_ticket("No ticket"));
    let r = d.drain(vec![approved(PR_A, &[]), approved(PR_B, &[]), approved(PR_C, &[])], &[]);
    assert_eq!(r.status, 0, "{}", r.err);
    assert_eq!(d.tasks(), ["First", "Second", "Tenth"]);
    let given = d.runs.borrow()[0].1.clone();
    assert_eq!(given, Some(SpecFile { path: d.ticket_path("s", "01-first"), text: ready_ticket("First") }));
    for (n, (name, pr)) in [("01-first", PR_A), ("02-second", PR_B), ("10-tenth", PR_C)].into_iter().enumerate() {
        assert_eq!(d.ticket_header("s", name), (some(SPEC_IN_REVIEW), Some(d.run_id(n)), some(pr)), "{name}");
    }
    assert_eq!(d.text("s"), ready("Spec"));
    assert_eq!(d.ticket_text("s", "notes"), ready_ticket("No ticket"));
    let tail = format!(
        "s/01-first: ready-for-agent -> in-review  {PR_A}\ns/02-second: ready-for-agent -> in-review  {PR_B}\n\
         s/10-tenth: ready-for-agent -> in-review  {PR_C}\nall specs cleared\n"
    );
    assert!(r.out.ends_with(&tail), "{}", r.out);
}

#[test]
fn a_ticket_waits_while_its_blockers_pull_request_is_open_and_runs_once_it_merged() {
    let d = Drain::new();
    d.spec("s", &ready("Spec"));
    d.ticket("s", "01-first", &ready_ticket("First"));
    d.ticket("s", "02-second", &spec_text("Second", Some(SPEC_READY), &["Blocked by: 01"]));
    let r = d.drain(vec![approved(PR_A, &[])], &[]);
    assert_eq!(r.status, EXIT_WAITING, "{}", r.err);
    assert_eq!(d.tasks(), ["First"]);
    assert_eq!(d.ticket_header("s", "02-second"), (some(SPEC_READY), None, None));
    assert!(r.out.ends_with(&format!("s/02-second waits on:\n    s/01-first: in-review  {PR_A}\n")), "{}", r.out);

    d.pull_request(PR_A, "OPEN", &[]);
    let r = d.drain(vec![], &[]);
    assert_eq!((r.status, d.tasks().len()), (EXIT_WAITING, 1), "{}", r.err);

    d.pull_request(PR_A, "MERGED", &[]);
    let r = d.drain(vec![approved(PR_B, &[])], &[]);
    assert_eq!(r.status, 0, "{}", r.err);
    assert_eq!(d.tasks(), ["First", "Second"]);
    assert_eq!(d.ticket_header("s", "01-first"), (resolved(1), Some(d.run_id(0)), some(PR_A)));
    assert_eq!(d.ticket_header("s", "02-second"), (some(SPEC_IN_REVIEW), Some(d.run_id(1)), some(PR_B)));
    assert!(r.out.contains(&format!("s/01-first: in-review -> resolved  {PR_A}\n")), "{}", r.out);
}

#[test]
fn a_spec_is_done_once_each_ticket_is_resolved_or_wontfix_which_frees_its_dependants() {
    let in_review = ["Run: 20261024-100000-aaaaaa", &format!("PR: {PR_A}")];
    for (other, done) in [(TICKET_WONTFIX, true), (TICKET_RESOLVED, true), (SPEC_FOR_HUMAN, false)] {
        let d = Drain::new();
        d.spec("s", &ready("Spec"));
        d.spec("t", &spec_text("Next", Some(SPEC_READY), &["Blocked by: s"]));
        d.ticket("s", "01-first", &spec_text("First", Some(SPEC_IN_REVIEW), &in_review));
        d.ticket("s", "02-second", &spec_text("Second", Some(&format!("{other} (by hand)")), &[]));
        d.pull_request(PR_A, "MERGED", &[]);
        let outcomes = if done { vec![approved(PR_B, &[])] } else { vec![] };
        let r = d.drain(outcomes, &[]);
        assert_eq!(d.ticket_header("s", "01-first").0, resolved(1), "{other}");
        if done {
            assert_eq!(r.status, 0, "{other}: {}", r.err);
            assert_eq!(d.header("s").0, some(SPEC_DONE));
            assert!(r.out.contains("s: ready-for-agent -> done\n"), "{}", r.out);
            assert_eq!(d.tasks(), ["Next"]);
        } else {
            assert_eq!(r.status, EXIT_WAITING, "{other}: {}", r.err);
            assert_eq!(d.header("s").0, some(SPEC_READY));
            assert!(r.out.ends_with("t waits on:\n    s: ready-for-agent\n"), "{}", r.out);
        }
    }
}

#[test]
fn a_ticket_whose_run_does_not_succeed_or_whose_pull_request_closed_is_the_humans() {
    let d = Drain::new();
    d.spec("s", &ready("Spec"));
    d.ticket("s", "01-first", &ready_ticket("First"));
    d.ticket("s", "02-second", &ready_ticket("Second"));
    let r = d.drain(vec![approved(PR_A, &["x.py"])], &[]);
    assert_eq!(r.status, EXIT_CONFLICT);
    assert_eq!(d.ticket_header("s", "01-first"), (some(SPEC_FOR_HUMAN), Some(d.run_id(0)), some(PR_A)));
    assert_eq!(d.ticket_header("s", "02-second"), (some(SPEC_READY), None, None));

    let d = Drain::new();
    d.spec("s", &ready("Spec"));
    d.ticket("s", "01-first", &spec_text("First", Some(SPEC_IN_REVIEW), &[&format!("PR: {PR_A}")]));
    d.pull_request(PR_A, "CLOSED", &[]);
    let r = d.drain(vec![], &[]);
    assert_eq!(r.status, 0, "{}", r.err);
    assert_eq!(d.ticket_header("s", "01-first").0, some(SPEC_FOR_HUMAN));
    assert_eq!(d.header("s").0, some(SPEC_READY));
}

#[test]
fn a_spec_without_tickets_or_not_ready_drains_as_before() {
    let d = Drain::new();
    d.spec("a-plain", &ready("Plain"));
    d.spec("b-no-tickets", &ready("No tickets"));
    d.ticket("b-no-tickets", "notes", &ready_ticket("Notes"));
    d.spec("c-held", &spec_text("Held", Some("needs-triage"), &[]));
    d.ticket("c-held", "01-first", &ready_ticket("Held ticket"));
    let r = d.drain(vec![approved(PR_A, &[]), approved(PR_B, &[])], &[]);
    assert_eq!(r.status, 0, "{}", r.err);
    assert_eq!(d.tasks(), ["Plain", "No tickets"]);
    assert_eq!(d.header("b-no-tickets"), (some(SPEC_IN_REVIEW), Some(d.run_id(1)), some(PR_B)));
    assert_eq!(d.ticket_text("c-held", "01-first"), ready_ticket("Held ticket"));
}

#[test]
fn a_dry_run_shows_the_ticket_runs_it_would_start() {
    let d = Drain::new();
    d.spec("p", &ready("Plain"));
    d.spec("s", &ready("Spec"));
    d.ticket("s", "01-first", &ready_ticket("First"));
    d.ticket("s", "02-second", &spec_text("Second", Some(SPEC_READY), &["Blocked by: 01"]));
    d.ticket("s", "03-third", &ready_ticket("Third"));
    let r = d.drain(vec![], &["--dry-run"]);
    assert_eq!(r.status, 0, "{}", r.err);
    assert_eq!(r.out, "would run, in order: p, s/01-first, s/03-third\n");
    assert_eq!(d.ticket_text("s", "01-first"), ready_ticket("First"));
}

#[test]
fn a_tickets_status_is_its_first_word_and_other_statuses_are_left_alone() {
    let d = Drain::new();
    d.spec("s", &ready("Spec"));
    d.ticket("s", "01-noted", &spec_text("Noted", Some("ready-for-agent (reviewed, review-02.md pass)"), &[]));
    let claimed = spec_text("Claimed", Some("claimed (2026-11-22, branch x)"), &[]);
    d.ticket("s", "02-claimed", &claimed);
    let r = d.drain(vec![approved(PR_A, &[])], &[]);
    assert_eq!(r.status, 0, "{}", r.err);
    assert_eq!(d.tasks(), ["Noted"]);
    assert_eq!(d.ticket_header("s", "01-noted").0, some(SPEC_IN_REVIEW));
    assert_eq!(d.ticket_text("s", "02-claimed"), claimed);
    assert_eq!(d.header("s").0, some(SPEC_READY));
}

#[test]
fn ticket_numbers_are_the_leading_digits_of_a_numbered_name() {
    assert_eq!(ticket_number("03-close-dialog"), Some(3));
    assert_eq!(ticket_number("12"), Some(12));
    assert_eq!(ticket_number("notes"), None);
    assert_eq!(ticket_number("3d-view"), None);
    assert_eq!(ticket_number("-03"), None);
}

#[test]
fn a_spec_with_tickets_made_ready_while_drain_runs_has_its_tickets_run_not_itself() {
    let d = Rc::new(Drain::new());
    d.spec("a", &ready("A"));
    d.spec("b", &spec_text("B", Some("needs-triage"), &[]));
    d.ticket("b", "01-first", &ready_ticket("B1"));
    let dd = Rc::clone(&d);
    let r = d.drain(vec![after(move || dd.spec("b", &ready("B")), approved(PR_A, &[])), approved(PR_B, &[])], &[]);
    assert_eq!(r.status, 0, "{}", r.err);
    assert_eq!(d.tasks(), ["A", "B1"]);
    assert_eq!(d.text("b"), ready("B"));
}

#[test]
fn a_spec_out_of_ready_starts_none_of_its_tickets_but_its_open_pull_requests_are_followed() {
    let d = Rc::new(Drain::new());
    d.spec("s", &ready("Spec"));
    d.ticket("s", "01-first", &ready_ticket("First"));
    d.ticket("s", "02-second", &ready_ticket("Second"));
    let dd = Rc::clone(&d);
    let hold = move || dd.spec("s", &spec_text("Spec", Some(SPEC_FOR_HUMAN), &[]));
    let r = d.drain(vec![after(hold, approved(PR_A, &[]))], &[]);
    assert_eq!((r.status, r.out.ends_with("all specs cleared\n")), (0, true), "{}{}", r.out, r.err);
    assert_eq!(d.tasks(), ["First"]);
    assert_eq!(d.ticket_header("s", "02-second"), (some(SPEC_READY), None, None));

    for (pr_state, settled) in [("MERGED", resolved(1)), ("CLOSED", some(SPEC_FOR_HUMAN))] {
        let d = Drain::new();
        d.spec("s", &spec_text("Spec", Some("needs-triage"), &[]));
        d.ticket("s", "01-first", &spec_text("First", Some(SPEC_IN_REVIEW), &[&format!("PR: {PR_A}")]));
        d.ticket("s", "02-second", &ready_ticket("Second"));
        d.pull_request(PR_A, pr_state, &[]);
        let r = d.drain(vec![], &[]);
        assert_eq!(r.status, 0, "{pr_state}: {}", r.err);
        assert_eq!(d.ticket_header("s", "01-first").0, settled, "{pr_state}");
        assert_eq!(d.tasks(), Vec::<String>::new());
    }
}

#[test]
fn a_specs_status_is_its_first_word_too() {
    let d = Drain::new();
    d.spec("a", &spec_text("Noted", Some("ready-for-agent (reviewed, review-02.md pass)"), &[]));
    d.spec("b", &spec_text("Ticketed", Some("ready-for-agent (tickets carry their own)"), &[]));
    d.ticket("b", "01-first", &ready_ticket("B1"));
    let r = d.drain(vec![approved(PR_A, &[]), approved(PR_B, &[])], &[]);
    assert_eq!(r.status, 0, "{}", r.err);
    assert_eq!(d.tasks(), ["Noted", "B1"]);
    assert_eq!(d.header("a").0, some(SPEC_IN_REVIEW));
}

#[test]
fn a_tickets_blockers_name_tickets_by_number_or_another_specs_whether_or_not_that_spec_is_ready() {
    let d = Drain::new();
    d.spec("a", &ready("A"));
    d.ticket("a", "01-x", &ready_ticket("A1"));
    d.spec("b", &spec_text("B", Some(SPEC_READY), &["Blocked by: c"]));
    d.ticket("b", "01-y", &spec_text("B1", Some(SPEC_READY), &["Blocked by: a/1 (same files)"]));
    d.ticket("b", "02-z", &spec_text("B2", Some(SPEC_READY), &["Blocked by: 01-y, c/04, d/02, e/03"]));
    d.spec("c", &spec_text("C", Some(SPEC_DONE), &[]));
    d.spec("d", &spec_text("D", Some("needs-triage"), &[]));
    d.ticket("d", "02-w", &spec_text("D2", Some(TICKET_RESOLVED), &[]));
    d.ticket("e", "03-v", &spec_text("E3", Some(TICKET_RESOLVED), &[]));
    let queue = read_queue(&d.dir()).unwrap();
    let read: Vec<(&str, &Kind, Vec<String>, Vec<String>)> =
        queue.iter().map(|q| (q.slug.as_str(), &q.kind, q.blockers.clone(), q.unread.clone())).collect();
    let (a, b) = (Kind::Ticket("a".into()), Kind::Ticket("b".into()));
    let (d_, e) = (Kind::Ticket("d".into()), Kind::Ticket("e".into()));
    let strings = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let unread = strings(&["its Blocked by: entry `c/04` names no ticket"]);
    assert_eq!(
        read,
        [
            ("a", &Kind::Ticketed, vec![], vec![]),
            ("a/01-x", &a, vec![], vec![]),
            ("b", &Kind::Ticketed, strings(&["c"]), vec![]),
            ("b/01-y", &b, strings(&["a/01-x"]), vec![]),
            ("b/02-z", &b, strings(&["b/01-y", "d/02-w", "e/03-v"]), unread),
            ("c", &Kind::Spec, vec![], vec![]),
            ("d", &Kind::Ticketed, vec![], vec![]),
            ("d/02-w", &d_, vec![], vec![]),
            ("e/03-v", &e, vec![], vec![]),
        ]
    );
}

#[test]
fn a_ticket_whose_blocker_drain_cannot_read_or_is_wontfix_is_held_for_the_human_and_the_drain_goes_on() {
    let cases = [
        ("Blocked by: 07", "its Blocked by: entry `07` names no ticket"),
        ("Blocked by: 04, 05; a decision on x", "its Blocked by: entry `05; a decision on x` names no ticket"),
        ("Blocked by: nosuch/01", "its Blocked by: entry `nosuch/01` names no ticket"),
        ("Blocked by: 02", "its Blocked by: entry `02` names more than one: s/02-a, s/02-b"),
        ("Blocked by: 03", "it is blocked by s/03-dropped, which is wontfix"),
    ];
    for (blocked_by, why) in cases {
        let d = Drain::new();
        d.spec("a", &ready("A"));
        d.ticket("a", "01-x", &ready_ticket("A1"));
        d.spec("s", &ready("Spec"));
        d.ticket("s", "01-held", &spec_text("Held", Some(SPEC_READY), &[blocked_by]));
        for name in ["02-a", "02-b", "04-four"] {
            d.ticket("s", name, &spec_text(name, Some(SPEC_IN_REVIEW), &[]));
        }
        d.ticket("s", "03-dropped", &spec_text("Dropped", Some(TICKET_WONTFIX), &[]));
        let r = d.drain(vec![approved(PR_A, &[])], &[]);
        assert_eq!(r.status, EXIT_ERROR, "{blocked_by}: {}", r.err);
        assert_eq!(d.tasks(), ["A1"], "{blocked_by}");
        assert!(r.out.ends_with(&format!("s/01-held needs the human: {why}\n")), "{blocked_by}: {}", r.out);
        assert_eq!(d.ticket_header("s", "01-held"), (some(SPEC_READY), None, None));
    }
}

#[test]
fn a_cycle_through_a_specs_tickets_stops_the_drain_before_any_run() {
    let d = Drain::new();
    d.spec("x", &ready("X"));
    d.ticket("x", "01-a", &spec_text("X1", Some(SPEC_READY), &["Blocked by: y/01"]));
    d.spec("y", &spec_text("Y", Some(SPEC_READY), &["Blocked by: x"]));
    d.ticket("y", "01-b", &ready_ticket("Y1"));
    let r = d.drain(vec![], &[]);
    assert_eq!(r.status, EXIT_ERROR);
    assert!(r.err.starts_with("error: the blockers form a cycle: x -> x/01-a -> y/01-b -> x\n"), "{}", r.err);
}

#[test]
fn a_blocked_by_line_naming_none_with_a_note_names_no_blocker() {
    let d = Drain::new();
    d.spec("s", &ready("Spec"));
    d.ticket("s", "01-first", &spec_text("First", Some(SPEC_READY), &["Blocked by: none (independent of 02)"]));
    d.ticket("s", "02-second", &spec_text("Second", Some(SPEC_READY), &["Blocked by: - (see the spec)"]));
    let r = d.drain(vec![approved(PR_A, &[]), approved(PR_B, &[])], &[]);
    assert_eq!(r.status, 0, "{}", r.err);
    assert_eq!(d.tasks(), ["First", "Second"]);
    assert_eq!(blocker_slugs(Some("None (independent)")), Vec::<String>::new());
}

#[test]
fn a_ticketed_spec_deleted_between_two_runs_leaves_its_ready_tickets_named_gone() {
    let d = Rc::new(Drain::new());
    d.spec("a", &ready("A"));
    d.spec("s", &ready("Spec"));
    d.ticket("s", "01-first", &ready_ticket("First"));
    let dd = Rc::clone(&d);
    let r = d.drain(vec![after(move || dd.remove("s"), approved(PR_A, &[]))], &[]);
    assert_eq!(r.status, EXIT_WAITING, "{}", r.err);
    assert_eq!(d.tasks(), ["A"]);
    assert!(!r.out.contains("all specs cleared"), "{}", r.out);
    assert!(r.out.ends_with(&format!("s/01-first: gone (its spec s is gone: {})\n", d.gone("s"))), "{}", r.out);
}

#[test]
fn a_dry_run_names_each_held_ticket_and_exits_as_the_drain_would() {
    let d = Drain::new();
    d.spec("s", &ready("Spec"));
    d.ticket("s", "01-first", &ready_ticket("First"));
    let held = spec_text("Held", Some(SPEC_READY), &["Blocked by: 07"]);
    d.ticket("s", "02-held", &held);
    let r = d.drain(vec![], &["--dry-run"]);
    assert_eq!(r.status, EXIT_ERROR, "{}", r.err);
    let why = "its Blocked by: entry `07` names no ticket";
    assert_eq!(r.out, format!("would run, in order: s/01-first\ns/02-held needs the human: {why}\n"));
    assert_eq!((d.ticket_text("s", "01-first"), d.ticket_text("s", "02-held")), (ready_ticket("First"), held));
}

#[test]
fn a_blocked_by_entry_names_a_ticket_by_its_whole_file_name_or_its_number() {
    let d = Drain::new();
    d.spec("s", &ready("Spec"));
    d.ticket("s", "02-new-name", &ready_ticket("Second"));
    d.ticket("s", "03-by-old-name", &spec_text("Third", Some(SPEC_READY), &["Blocked by: 02-old-name"]));
    d.ticket("s", "04-by-number", &spec_text("Fourth", Some(SPEC_READY), &["Blocked by: 02"]));
    let noted = "Blocked by: 02 (same file, also the UI), 02-new-name";
    d.ticket("s", "05-noted", &spec_text("Fifth", Some(SPEC_READY), &[noted]));
    d.ticket("s", "06-dash", &spec_text("Sixth", Some(SPEC_READY), &["Blocked by: - 02"]));
    d.ticket("s", "07-none-yet", &spec_text("Seventh", Some(SPEC_READY), &["Blocked by: none yet, 02"]));
    d.ticket("s", "08-note-only", &spec_text("Eighth", Some(SPEC_READY), &["Blocked by: (after 02 lands)"]));
    d.ticket("s", "09-with-md", &spec_text("Ninth", Some(SPEC_READY), &["Blocked by: 02-new-name.md"]));
    let queue = read_queue(&d.dir()).unwrap();
    let read: Vec<(&str, Vec<String>, Vec<String>)> =
        queue.iter().skip(2).map(|q| (q.slug.as_str(), q.blockers.clone(), q.unread.clone())).collect();
    let strings = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        read,
        [
            ("s/03-by-old-name", vec![], strings(&["its Blocked by: entry `02-old-name` names no ticket"])),
            ("s/04-by-number", strings(&["s/02-new-name"]), vec![]),
            ("s/05-noted", strings(&["s/02-new-name", "s/02-new-name"]), vec![]),
            ("s/06-dash", vec![], strings(&["its Blocked by: entry `- 02` names no ticket"])),
            (
                "s/07-none-yet",
                strings(&["s/02-new-name"]),
                strings(&["its Blocked by: entry `none yet` names no ticket"])
            ),
            ("s/08-note-only", vec![], strings(&["its Blocked by: entry `(after 02 lands)` names no ticket"])),
            ("s/09-with-md", strings(&["s/02-new-name"]), vec![]),
        ]
    );
    // A spec's line is read the same way: a note's comma parts nothing, and `none yet` is no `none`.
    assert_eq!(blocker_slugs(Some("base (same files, and more)")), ["base"]);
    assert_eq!(blocker_slugs(Some("none yet, base")), ["none yet", "base"]);
    assert_eq!(blocker_slugs(Some("- base")), ["- base"]);
    assert_eq!(blocker_slugs(Some("(after base lands)")), ["(after base lands)"]);
    d.spec("t", &spec_text("T", Some(SPEC_READY), &["Blocked by: none yet, s"]));
    let e = read_queue(&d.dir()).unwrap_err();
    assert!(e.0.starts_with("spec t is blocked by none yet, which has no spec.md"), "{e}");
}

#[test]
fn a_spec_blocked_by_a_wontfix_spec_is_held_for_the_human_as_a_ticket_is() {
    let d = Drain::new();
    d.spec("a", &spec_text("A", Some(TICKET_WONTFIX), &[]));
    d.spec("b", &spec_text("B", Some(SPEC_READY), &["Blocked by: a"]));
    d.spec("c", &ready("C"));
    let r = d.drain(vec![approved(PR_A, &[])], &[]);
    assert_eq!(r.status, EXIT_ERROR, "{}", r.err);
    assert_eq!(d.tasks(), ["C"]);
    assert!(r.out.ends_with("b needs the human: it is blocked by a, which is wontfix\n"), "{}", r.out);
    assert_eq!(d.header("b"), (some(SPEC_READY), None, None));
}

#[test]
fn a_ticketed_spec_is_done_once_its_last_ticket_merges_whatever_its_status() {
    for status in [SPEC_FOR_HUMAN, "needs-triage (waiting on x)"] {
        let d = Drain::new();
        d.spec("s", &spec_text("Spec", Some(status), &[]));
        d.ticket("s", "01-first", &spec_text("First", Some(SPEC_IN_REVIEW), &[&format!("PR: {PR_A}")]));
        d.ticket("s", "02-second", &spec_text("Second", Some(TICKET_WONTFIX), &[]));
        d.pull_request(PR_A, "MERGED", &[]);
        let r = d.drain(vec![], &[]);
        assert_eq!(r.status, 0, "{status}: {}", r.err);
        assert_eq!(d.header("s").0, some(SPEC_DONE), "{status}");
        let was = status_word(status);
        assert!(r.out.contains(&format!("s: {was} -> done\n")), "{status}: {}", r.out);
    }
}

#[test]
fn a_wontfix_or_held_ticketed_spec_is_not_made_done() {
    // Closed by the human with its tickets: nothing merged, and what it blocks stays held.
    let d = Drain::new();
    d.spec("s", &spec_text("Spec", Some(TICKET_WONTFIX), &[]));
    d.ticket("s", "01-first", &spec_text("First", Some(TICKET_WONTFIX), &[]));
    d.spec("b", &spec_text("B", Some(SPEC_READY), &["Blocked by: s"]));
    let r = d.drain(vec![], &[]);
    assert_eq!(r.status, EXIT_ERROR, "{}", r.err);
    assert_eq!(d.header("s").0, some(TICKET_WONTFIX));
    assert!(d.tasks().is_empty());
    // Held by a wontfix blocker, its tickets resolved before that.
    let d = Drain::new();
    d.spec("w", &spec_text("W", Some(TICKET_WONTFIX), &[]));
    d.spec("s", &spec_text("Spec", Some(SPEC_READY), &["Blocked by: w"]));
    d.ticket("s", "01-first", &spec_text("First", resolved(1).as_deref(), &[]));
    let r = d.drain(vec![], &[]);
    assert_eq!(r.status, EXIT_ERROR, "{}", r.err);
    assert_eq!(d.header("s").0, some(SPEC_READY));
    assert!(r.out.ends_with("s needs the human: it is blocked by w, which is wontfix\n"), "{}", r.out);
}

#[test]
fn a_held_specs_tickets_run_lines_are_not_read_and_the_other_specs_drain() {
    let stale = ["Run: 20261024-100000-aaaaaa"];
    // Held by its status, and held for the human by a wontfix blocker.
    for (status, blocked_by, exit) in [("needs-triage", "Blocked by: -", 0), (SPEC_READY, "Blocked by: w", EXIT_ERROR)]
    {
        let d = Drain::new();
        d.spec("a", &ready("A"));
        d.spec("s", &spec_text("Spec", Some(status), &[blocked_by]));
        d.ticket("s", "01-stale", &spec_text("Stale", Some(SPEC_READY), &stale));
        d.spec("w", &spec_text("W", Some(TICKET_WONTFIX), &[]));
        let r = d.drain(vec![approved(PR_A, &[])], &[]);
        assert_eq!(r.status, exit, "{status}: {}{}", r.out, r.err);
        assert_eq!(d.tasks(), ["A"], "{status}");
        assert_eq!(d.ticket_header("s", "01-stale"), (some(SPEC_READY), some(stale[0][5..].trim()), None));
    }
    // Nor is the Run: line of a ticket whose directory has no spec.md, which never runs.
    let d = Drain::new();
    d.spec("a", &ready("A"));
    d.ticket("orphan", "01-stale", &spec_text("Stale", Some(SPEC_READY), &stale));
    let r = d.drain(vec![approved(PR_A, &[])], &[]);
    assert_eq!((r.status, d.tasks()), (0, vec!["A".to_string()]), "{}", r.err);
    // A ticket under a spec drain runs still stops the drain.
    let d = Drain::new();
    d.spec("s", &ready("Spec"));
    d.ticket("s", "01-stale", &spec_text("Stale", Some(SPEC_READY), &stale));
    let r = d.drain(vec![], &[]);
    assert_eq!(r.status, EXIT_ERROR);
    assert!(r.err.contains("ticket s/01-stale: run 20261024-100000-aaaaaa, on its Run: line"), "{}", r.err);
}

#[test]
fn an_unreadable_ticket_holds_its_spec_for_the_human_and_the_other_specs_drain() {
    let d = Drain::new();
    d.spec("a", &ready("A"));
    d.spec("s", &ready("Spec"));
    d.ticket("s", "01-ok", &ready_ticket("Fine"));
    std::fs::write(d.ticket_path("s", "02-bad"), b"Status: \xff\n").unwrap();
    let r = d.drain(vec![approved(PR_A, &[])], &[]);
    assert_eq!(r.status, EXIT_ERROR, "{}", r.err);
    assert_eq!(d.tasks(), ["A"]);
    let why = format!("spec file {} is not UTF-8 text", d.ticket_path("s", "02-bad"));
    assert!(r.out.contains(&format!("s needs the human: {why}")), "{}", r.out);
    assert!(r.out.contains("s/01-ok needs the human: its spec s needs the human\n"), "{}", r.out);
    assert_eq!(d.ticket_text("s", "01-ok"), ready_ticket("Fine"));
    // Whatever the spec's status.
    let d = Drain::new();
    d.spec("s", &spec_text("Spec", Some(SPEC_IN_REVIEW), &[]));
    d.ticket("s", "01-ok", &ready_ticket("Fine"));
    std::fs::write(d.ticket_path("s", "02-bad"), b"Status: \xff\n").unwrap();
    let r = d.drain(vec![], &[]);
    assert_eq!(r.status, EXIT_ERROR, "{}", r.err);
    let why = format!("spec file {} is not UTF-8 text", d.ticket_path("s", "02-bad"));
    assert!(r.out.contains(&format!("s needs the human: {why}")), "{}", r.out);
    // An unreadable spec.md still stops the drain.
    std::fs::write(d.path("s"), b"Status: \xff\n").unwrap();
    let r = d.drain(vec![], &[]);
    assert_eq!(r.status, EXIT_ERROR);
    assert!(r.err.starts_with(&format!("error: spec file {} is not UTF-8 text", d.path("s"))), "{}", r.err);
}
