//! Synthetic source delivery cases. Git runs with the isolated test environment.

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Output;

use common::{Sandbox, copy_manifest, json, stdout, write};

struct Workspace {
    sandbox: Sandbox,
    kb: PathBuf,
    host: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let sandbox = Sandbox::new();
        let kb = sandbox.path().join("kb");
        let host = sandbox.path().join("host");
        copy_manifest(&kb);
        sandbox.init_repo(&host);
        Self { sandbox, kb, host }
    }

    fn read(&self, args: &[&str]) -> Output {
        let mut command = vec!["--root", self.kb.to_str().unwrap(), "read"];
        command.extend_from_slice(args);
        self.sandbox.kb(&self.host, &command, &[])
    }
}

#[test]
fn read_preserves_source_and_resolves_a_unique_suffix_or_stem() {
    let world = Workspace::new();
    let path = "src/feature/Widget.kt";
    let long = "x".repeat(240);
    write(
        &world.host.join(path),
        &format!(
            "class Widget {{\r\n\tval message = \"two  spaces\"  \r\n\r\n    // {long}\r\n}}\r\n"
        ),
    );
    let expected = format!("{path}\n2:\tval message = \"two  spaces\"  \n3:\n4:    // {long}\n");
    for name in [
        path,
        "./src/feature/Widget.kt",
        "feature/Widget.kt",
        "Widget",
    ] {
        let output = world.read(&[name, "2", "4"]);
        assert!(output.status.success(), "{}", common::stderr(&output));
        assert_eq!(stdout(&output), expected);
    }
    let output = world.read(&["Widget", "--json"]);
    assert!(output.status.success());
    let result = json(&output);
    assert_eq!(result["command"], "read");
    assert_eq!(result["result"]["path"], path);
    assert_eq!(result["result"]["last_line"], 5);
    assert_eq!(result["result"]["total_lines"], 5);
    assert_eq!(
        result["result"]["lines"][1],
        "\tval message = \"two  spaces\"  "
    );
    assert!(result["result"]["next_line"].is_null());
}

#[test]
fn read_takes_several_ranges_in_one_call() {
    let world = Workspace::new();
    let numbered = |n: usize| (1..=n).map(|i| format!("line {i}\n")).collect::<String>();
    write(&world.host.join("src/A.kt"), &numbered(30));
    write(&world.host.join("src/B.kt"), &numbered(300));

    let output = world.read(&["A", "2", "3", "src/B.kt", "10", "11"]);
    assert!(output.status.success(), "{}", common::stderr(&output));
    assert_eq!(
        stdout(&output),
        "src/A.kt\n2:line 2\n3:line 3\nsrc/B.kt\n10:line 10\n11:line 11\n"
    );

    // A range that fails is reported in place and does not cost the others.
    let output = world.read(&["A", "29", "40", "Missing.kt", "1", "5", "B", "300", "300"]);
    assert!(output.status.success(), "{}", common::stderr(&output));
    let text = stdout(&output);
    assert!(
        text.starts_with("src/A.kt\n29:line 29\n30:line 30\n-- end of file (30 lines)\n-- Missing.kt 1 5: no file matches `Missing.kt` among the "),
        "{text}"
    );
    assert!(
        text.ends_with("src/B.kt\n300:line 300\n-- end of file (300 lines)\n"),
        "{text}"
    );
    let result = json(&world.read(&["A", "1", "2", "Missing.kt", "1", "5", "--json"]));
    assert_eq!(result["result"]["ranges"][0]["path"], "src/A.kt");
    assert_eq!(result["result"]["ranges"][0]["last_line"], 2);
    assert!(
        result["result"]["ranges"][1]["error"].is_string(),
        "{result:#}"
    );

    // Each range keeps the 80-line limit; the call as a whole stops at 240 lines and names
    // the ranges it did not read.
    let output = world.read(&[
        "B", "1", "80", "B", "81", "160", "B", "161", "240", "B", "241", "300", "A", "1", "5",
    ]);
    let text = stdout(&output);
    assert_eq!(
        text.lines()
            .filter(|l| l.starts_with(|c: char| c.is_ascii_digit()))
            .count(),
        240
    );
    assert!(
        text.ends_with("-- not read, 240 lines per call: B 241 300; A 1 5\n"),
        "{text}"
    );

    // Two numbers alone are another range of the file named last.
    let pairs = stdout(&world.read(&["B", "1", "2", "5", "6", "A", "1", "1", "3", "3"]));
    let grouped =
        stdout(&world.read(&["B", "1", "2", "B", "5", "6", "A", "1", "1", "A", "3", "3"]));
    assert_eq!(pairs, grouped);
    assert_eq!(pairs.matches("src/B.kt\n").count(), 2, "{pairs}");

    // A range longer than the window is cut at 80 lines inside a call of several, and
    // says where to resume; a call takes at most twelve ranges.
    let text = stdout(&world.read(&["B", "1", "100", "A", "1", "2"]));
    assert!(
        text.contains("80:line 80\n-- truncated; resume at line 81 through 100\n"),
        "{text}"
    );
    assert!(!text.contains("\n81:"), "{text}");
    let mut thirteen = vec!["A", "1", "1"];
    for _ in 0..12 {
        thirteen.extend(["1", "1"]);
    }
    let output = world.read(&thirteen);
    assert_eq!(
        output.status.code(),
        Some(64),
        "{}",
        common::stderr(&output)
    );
    assert!(common::stderr(&output).contains("one call reads at most 12 ranges"));

    // When every range fails the call fails; a malformed group is a usage error.
    let output = world.read(&["Missing.kt", "1", "5", "Other.kt", "1", "5"]);
    assert_eq!(output.status.code(), Some(16));
    let output = world.read(&["A", "1", "5", "B", "7"]);
    assert_eq!(
        output.status.code(),
        Some(64),
        "{}",
        common::stderr(&output)
    );
}

#[test]
fn read_marks_exact_continuation_for_line_and_byte_limits() {
    let world = Workspace::new();
    let text: String = (1..=95).map(|line| format!("statement {line}\n")).collect();
    write(&world.host.join("source.txt"), &text);
    let output = world.read(&["source.txt", "5", "88", "--json"]);
    assert!(output.status.success());
    let result = json(&output);
    let excerpt = &result["result"];
    assert_eq!(excerpt["first_line"], 5);
    assert_eq!(excerpt["last_line"], 84);
    assert_eq!(excerpt["next_line"], 85);
    assert_eq!(excerpt["lines"].as_array().unwrap().len(), 80);
    assert_eq!(excerpt["lines"][79], "statement 84");
    assert_eq!(excerpt["total_lines"], 95);

    // The continuation stops at the last line the file has, and an excerpt that reaches
    // the end says so: neither invites a read past the file.
    let clipped = stdout(&world.read(&["source.txt", "10", "200"]));
    assert!(
        clipped.ends_with(
            "89:statement 89\n-- truncated; resume at line 90 through 95 (end of file)\n"
        ),
        "{clipped}"
    );
    let tail = stdout(&world.read(&["source.txt", "90", "200"]));
    assert!(
        tail.ends_with("95:statement 95\n-- end of file (95 lines)\n"),
        "{tail}"
    );
    let inside = stdout(&world.read(&["source.txt", "90", "94"]));
    assert!(inside.ends_with("94:statement 94\n"), "{inside}");

    let body = "x".repeat(20_000);
    write(&world.host.join("wide.txt"), &format!("{body}\n{body}\n"));
    let output = world.read(&["wide.txt", "1", "2"]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        format!("wide.txt\n1:{body}\n-- truncated; resume at line 2 through 2 (end of file)\n")
    );
    let output = world.read(&["wide.txt", "2", "2"]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        format!("wide.txt\n2:{body}\n-- end of file (2 lines)\n")
    );
}

#[test]
fn read_distinguishes_absence_and_ambiguity_and_prefers_the_exact_path() {
    let world = Workspace::new();
    write(&world.host.join("src/One/Item.kt"), "one\n");
    write(&world.host.join("src/Two/Item.kt"), "two\n");
    write(&world.host.join("src/Three/OnlyIgnored.kt"), "ignore\n");
    write(&world.host.join(".gitignore"), "src/Three/\n");
    let ambiguous = world.read(&["Item", "--json"]);
    assert!(!ambiguous.status.success());
    let error = json(&ambiguous);
    assert_eq!(error["error"]["code"], "INVALID_INPUT");
    let message = error["error"]["message"].as_str().unwrap();
    assert!(message.contains("src/One/Item.kt, src/Two/Item.kt"));
    write(&world.host.join("Item.kt"), "exact\n");
    let output = world.read(&["Item.kt"]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        "Item.kt\n1:exact\n-- end of file (1 line)\n"
    );
    let missing = json(&world.read(&["OnlyIgnored", "--json"]));
    assert_eq!(missing["error"]["code"], "NOT_FOUND");
    // The miss covered every listed file, so another spelling is not worth a call.
    assert_eq!(
        missing["error"]["message"],
        "no file matches `OnlyIgnored` among the 4 file(s) of this checkout"
    );
    let after_end = json(&world.read(&["Item.kt", "2", "--json"]));
    assert_eq!(after_end["error"]["code"], "NOT_FOUND");
    assert_eq!(
        after_end["error"]["message"],
        "`Item.kt` ends at line 1; there is no line 2"
    );
    let reversed = world.read(&["Item.kt", "3", "2", "--json"]);
    assert_eq!(json(&reversed)["error"]["code"], "INVALID_INPUT");
}

#[test]
fn read_refuses_traversal_git_metadata_and_binary_content() {
    let world = Workspace::new();
    write(&world.sandbox.path().join("outside.txt"), "not source\n");
    for path in ["../outside.txt", ".git/config"] {
        let output = world.read(&[path, "--json"]);
        assert_eq!(json(&output)["error"]["code"], "UNSAFE_PATH");
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(world.sandbox.path(), world.host.join("link")).unwrap();
        let output = world.read(&["link/outside.txt", "--json"]);
        assert_eq!(json(&output)["error"]["code"], "UNSAFE_PATH");
    }
    for invalid in [b"\xff\n".as_slice(), b"text\0\n"] {
        fs::write(world.host.join("binary"), invalid).unwrap();
        let output = world.read(&["binary", "--json"]);
        assert_eq!(json(&output)["error"]["code"], "INVALID_INPUT");
    }
}
