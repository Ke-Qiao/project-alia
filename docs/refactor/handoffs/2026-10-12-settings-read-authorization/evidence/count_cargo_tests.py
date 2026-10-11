#!/usr/bin/env python3
"""Count a completed, sequential Cargo test log without child-process inflation.

Only the LAST libtest summary under each Cargo Running / Doc-tests heading is
counted. Earlier summaries and extra `running N tests` lines are retained as
nested harness evidence. No Cargo command, database, or network is used.
"""

import argparse
import hashlib
import json
from pathlib import Path
import re
import shlex


ANSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
HEADING = re.compile(r"^\s*(Running|Doc-tests)\s+(.+?)\s*$")
START = re.compile(r"^running (\d+) tests?\s*$")
SUMMARY = re.compile(
    r"^test result: (?P<status>ok|FAILED)\. "
    r"(?P<passed>\d+) passed; (?P<failed>\d+) failed; "
    r"(?P<ignored>\d+) ignored; (?P<measured>\d+) measured; "
    r"(?P<filtered_out>\d+) filtered out; finished in (?P<seconds>[\d.]+)s\s*$"
)
TEST = re.compile(r"^test (.+?) \.\.\. (ok|FAILED|ignored)(?:, (.*))?\s*$")
COUNTS = ("passed", "failed", "ignored", "measured", "filtered_out")


def total(summaries):
    summaries = tuple(summaries)
    return {key: sum(summary[key] for summary in summaries) for key in COUNTS}


def parse_log(path):
    raw = path.read_bytes()
    targets, orphans, errors, issues = [], [], [], []
    early_starts = []
    delayed_tails = []
    pending_heading = None
    previous_nonblank = None
    current = None
    for number, original in enumerate(raw.decode("utf-8", errors="replace").splitlines(), 1):
        line = ANSI.sub("", original)
        if pending_heading is not None and line.strip():
            match = SUMMARY.match(line)
            if match and sum(int(match[k]) for k in COUNTS if k != "filtered_out") == current["harness_starts"][0]["selected"]:
                item = {key: int(match[key]) for key in COUNTS}
                item.update(status=match["status"], seconds=float(match["seconds"]), line=number)
                current["summaries"].append(item)
                delayed_tails.append({"heading_line": pending_heading["heading_line"], "summary_line": number, "previous_target": current["target"]})
                current = pending_heading
                targets.append(current)
                pending_heading = None
                previous_nonblank = number
                continue
            if not TEST.match(line):
                issues.append(f"line {number}: ambiguous output after an early next-target heading")
                current = pending_heading
                targets.append(current)
                pending_heading = None
        if match := HEADING.match(line):
            if current is not None and current["harness_starts"] and not current["summaries"]:
                pending_heading = {"kind": match[1], "target": match[2], "heading_line": number,
                                   "harness_starts": [], "summaries": [], "nonpassing_test_lines": []}
                previous_nonblank = number
                continue
            carried_start = None
            # Docker can deliver stdout's start before stderr's Cargo heading.
            if (current is not None and current["summaries"] and current["harness_starts"]
                    and current["harness_starts"][-1]["line"] == previous_nonblank
                    and current["harness_starts"][-1]["line"] > current["summaries"][-1]["line"]):
                carried_start = current["harness_starts"].pop()
                early_starts.append({"start_line": carried_start["line"], "heading_line": number,
                                     "selected": carried_start["selected"], "target": match[2]})
            current = {
                "kind": match[1], "target": match[2], "heading_line": number,
                "harness_starts": [], "summaries": [], "nonpassing_test_lines": [],
            }
            if carried_start is not None:
                current["harness_starts"].append(carried_start)
            targets.append(current)
        elif match := SUMMARY.match(line):
            item = {key: int(match[key]) for key in COUNTS}
            item.update(status=match["status"], seconds=float(match["seconds"]), line=number)
            (current["summaries"] if current is not None else orphans).append(item)
        elif line.startswith("test result:"):
            issues.append(f"line {number}: unrecognized libtest summary")
        elif current is not None and (match := START.match(line)):
            current["harness_starts"].append({"line": number, "selected": int(match[1])})
        elif current is not None and (match := TEST.match(line)) and match[2] != "ok":
            current["nonpassing_test_lines"].append({
                "line": number, "name": match[1], "status": match[2], "reason": match[3],
            })
        elif line.startswith("error:"):
            errors.append({"line": number, "text": line})
        if line.strip():
            previous_nonblank = number

    if pending_heading is not None:
        targets.append(pending_heading)
        issues.append("next-target heading without completed preceding tail")
    counted, excluded = [], []
    for target in targets:
        summaries = target.pop("summaries")
        starts = target.pop("harness_starts")
        target["outer_harness_start"] = starts[0] if starts else None
        target["nested_harness_starts"] = starts[1:]
        target["excluded_earlier_summaries"] = summaries[:-1]
        target["counted_summary"] = summaries[-1] if summaries else None
        excluded.extend(summaries[:-1])
        if not summaries:
            issues.append(f"line {target['heading_line']}: target has no final summary")
            continue
        last = summaries[-1]
        observed = sum(last[key] for key in COUNTS if key != "filtered_out")
        if not starts or starts[0]["selected"] != observed:
            issues.append(
                f"line {last['line']}: last summary does not match the outer harness count; "
                "do not treat a possible child summary as completed parent evidence"
            )
        if (last["status"] == "ok") != (last["failed"] == 0):
            issues.append(f"line {last['line']}: inconsistent summary status / failed count")
        counted.append(last)
    if orphans:
        issues.append("summaries outside a Cargo target boundary were excluded")
    if not targets:
        issues.append("no Cargo Running / Doc-tests boundaries found")

    return {
        "log_path": str(path.resolve()),
        "log_sha256": hashlib.sha256(raw).hexdigest(),
        "counting_rule": "last libtest summary per sequential Cargo Running / Doc-tests boundary",
        "target_count": len(targets),
        "running_targets": sum(t["kind"] == "Running" for t in targets),
        "doctest_targets": sum(t["kind"] == "Doc-tests" for t in targets),
        "totals": total(counted),
        "running_totals": total(t["counted_summary"] for t in targets
                                if t["kind"] == "Running" and t["counted_summary"]),
        "doctest_totals": total(t["counted_summary"] for t in targets
                                if t["kind"] == "Doc-tests" and t["counted_summary"]),
        "excluded_nested_summary_count": len(excluded),
        "excluded_nested_totals": total(excluded),
        "nested_harness_start_count": sum(len(t["nested_harness_starts"]) for t in targets),
        "orphan_summaries": orphans,
        "cargo_errors": errors,
        "parse_issues": issues,
        "targets": targets,
        "adjacent_pre_heading_starts": early_starts,
        "delayed_previous_target_tails": delayed_tails,
    }


def invocation(metadata):
    command = metadata.get("command", [])
    args = shlex.split(command) if isinstance(command, str) else command
    # Test-runner arguments after `--` are not Cargo feature selectors.
    cargo = args[:args.index("--")] if "--" in args else args
    explicit = []
    target = None
    for index, arg in enumerate(cargo):
        if arg in ("--features", "-F") and index + 1 < len(cargo):
            explicit.extend(re.split(r"[\s,]+", cargo[index + 1]))
        elif arg.startswith("--features="):
            explicit.extend(re.split(r"[\s,]+", arg.split("=", 1)[1]))
        elif arg == "--target" and index + 1 < len(cargo):
            target = cargo[index + 1]
        elif arg.startswith("--target="):
            target = arg.split("=", 1)[1]
    return {
        "command": args,
        "workspace_requested": "--workspace" in cargo,
        "all_features_requested": "--all-features" in cargo,
        "default_features_disabled": "--no-default-features" in cargo,
        "explicit_features": sorted(set(filter(None, explicit))),
        "target_argument": target,
        "evidence_limit": "recorded command only; not a resolved feature graph or hardware validation",
    }


def render(report):
    lines = [f"Log: {report['log_path']}", f"SHA-256: {report['log_sha256']}",
             f"Assessment: {report['assessment']}",
             f"Targets: {report['running_targets']} Running + {report['doctest_targets']} Doc-tests",
             "Counted outer totals: " + json.dumps(report["totals"]),
             "Excluded nested summaries: " + str(report["excluded_nested_summary_count"])
             + "; totals: " + json.dumps(report["excluded_nested_totals"]),
             f"Extra nested harness starts: {report['nested_harness_start_count']}",
             "Earlier summaries are excluded, not added to passed / filtered totals.", ""]
    if "invocation" in report:
        lines.extend(["Recorded invocation: " + json.dumps(report["invocation"]),
                      "Cargo exit code: " + str(report["run_metadata"].get("exit_code")), ""])
    lines.append("line\tstatus\tpassed\tfailed\tignored\tmeasured\tfiltered\ttarget")
    for target in report["targets"]:
        summary = target["counted_summary"]
        values = [str(summary["line"]), summary["status"],
                  *(str(summary[key]) for key in COUNTS)] if summary else ["?", "MISSING", *("?" for _ in COUNTS)]
        lines.append("\t".join(values + [f"{target['kind']} {target['target']}"]))
        for item in target["excluded_earlier_summaries"]:
            lines.append(f"  EXCLUDED nested summary line {item['line']}: {json.dumps(item)}")
        for item in target["nested_harness_starts"]:
            lines.append(f"  Nested harness start line {item['line']}: {item['selected']} selected; "
                         "child may exit intentionally without a summary")
        for item in target["nonpassing_test_lines"]:
            lines.append(f"  Observed {item['status']} line {item['line']}: {item['name']}"
                         + (f"; {item['reason']}" if item["reason"] else ""))
    for item in report["adjacent_pre_heading_starts"]:
        lines.append("Adjacent stdout start before Cargo heading: " + json.dumps(item))
    for item in report["delayed_previous_target_tails"]:
        lines.append("Delayed stdout tail after next Cargo heading: " + json.dumps(item))
    for issue in report["parse_issues"]:
        lines.append("PARSE ISSUE: " + issue)
    lines.extend(["", "Limits: sequential Cargo log required; nested Cargo Running headings or interleaved",
                  "independent Cargo invocations require separate logs. Test lines may include child output;",
                  "only outer summaries contribute counts. No sidecar means overall run completion is unconfirmed."])
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path)
    parser.add_argument("--result", type=Path, help="run_stage sidecar with command, exit_code and finish time")
    parser.add_argument("--json", type=Path, help="write detailed machine-readable report")
    parser.add_argument("--text", type=Path, help="write human-readable target table")
    options = parser.parse_args()
    report = parse_log(options.log)
    metadata = json.loads(options.result.read_text()) if options.result else {}
    if options.result:
        report["run_metadata"] = metadata
        report["invocation"] = invocation(metadata)
    confirmed = bool(metadata.get("finished_at")) and isinstance(metadata.get("exit_code"), int)
    failed = report["totals"]["failed"] > 0 or bool(report["cargo_errors"]) or (
        confirmed and metadata["exit_code"] != 0
    )
    drifted = bool(metadata.get("source_drift"))
    report["run_completion_confirmed"] = confirmed
    report["source_drift_recorded"] = drifted
    report["assessment"] = (
        "INVALID_OR_INCOMPLETE_PARSE" if report["parse_issues"] else
        "FAILED" if failed else "SOURCE_DRIFT" if drifted else
        "PASSED_RECORDED_RUN" if confirmed else "PASSED_OBSERVED_TARGETS_ONLY"
    )
    rendered = render(report)
    for destination in (options.json, options.text):
        if destination and destination.resolve() in {
            options.log.resolve(), options.result.resolve() if options.result else None
        }:
            parser.error("report destination must not overwrite input evidence")
    if options.json:
        options.json.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    if options.text:
        options.text.write_text(rendered)
    print(rendered, end="")
    # 0: completed passing recorded run; 1: recorded failure; 2: invalid / incomplete evidence.
    return 2 if report["parse_issues"] or not confirmed or drifted else 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
