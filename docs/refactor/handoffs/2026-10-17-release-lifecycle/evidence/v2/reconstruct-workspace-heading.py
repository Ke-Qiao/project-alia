"""One recorded stdout/stderr split; derived evidence, never a Cargo rerun."""
from pathlib import Path
import hashlib
import json
import subprocess
import sys

ROOT = Path(__file__).resolve().parent
EVIDENCE = ROOT / 'evidence'
ORIGINAL_SHA256 = '304a66e66027fb47df88960c31f4657fb3a92dd24ce493e2bd8e30a8345ab08b'
BLOCK = (
    b'     Running unittests src/main.rs (/cache/target/debug/deps/alia_server-c5a3f1c9bba3e839)\n'
    b'\n'
    b'running 0 tests\n'
    b'     Running\n'
    b'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n'
    b'\n'
    b' tests/appraisal_provenance.rs (/cache/target/debug/deps/appraisal_provenance-117dda89bb0a0a84)\n'
    b'\n'
    b'running 20 tests\n'
)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def reconstruct(raw):
    # No generic whitespace cleanup or fallback for other interleavings.
    assert raw.count(BLOCK) == 1, 'requires the one exact observed block'
    assert raw.count(b'     Running\n') == 1, 'requires one split prefix only'
    lines = BLOCK.splitlines(keepends=True)
    replacement = b''.join(lines[:3] + lines[4:6] + [lines[3][:-1] + lines[6]] + lines[7:])
    return raw.replace(BLOCK, replacement, 1)


def self_test():
    exact = reconstruct(b'before\n' + BLOCK + b'after\n')
    assert exact.startswith(b'before\n') and exact.endswith(b'after\n')
    assert exact.count(b'     Running tests/appraisal_provenance.rs ') == 1
    assert exact.index(b'test result: ok. 0 passed') < exact.index(b'     Running tests/appraisal_provenance.rs ')
    rejected = [
        BLOCK + BLOCK,
        BLOCK.replace(b'0 passed; 0 failed', b'1 passed; 0 failed'),
        BLOCK.replace(b' tests/appraisal_provenance.rs ', b' tests/unknown.rs '),
        BLOCK + b'     Running\n',
    ]
    for raw in rejected:
        try:
            reconstruct(raw)
        except AssertionError:
            continue
        raise AssertionError('unexpected shape was accepted')
    return {'cases': 5, 'passed': 5,
            'scope': 'exact bounded split accepted; duplicate block, changed summary, other target, extra split rejected'}


def main():
    self_tests = self_test()
    original = EVIDENCE / 'workspace-default.log'
    invalid_json = EVIDENCE / 'workspace-counts.json'
    invalid_text = EVIDENCE / 'workspace-counts.txt'
    sidecar = EVIDENCE / 'workspace-default.result.json'
    counter = ROOT / 'count_cargo_tests.py'
    derived = EVIDENCE / 'workspace-default.derived.log'
    confirmed_json = EVIDENCE / 'workspace-counts-confirmed.json'
    confirmed_text = EVIDENCE / 'workspace-counts-confirmed.txt'
    report_path = EVIDENCE / 'reconstruction.json'
    outputs = [derived, confirmed_json, confirmed_text, report_path]
    assert not any(path.exists() for path in outputs), 'derived outputs must be new'
    inputs = [original, invalid_json, invalid_text, sidecar, counter]
    before = {str(path.relative_to(ROOT)): sha(path.read_bytes()) for path in inputs}
    raw = original.read_bytes()
    assert sha(raw) == ORIGINAL_SHA256, 'not the reviewed original log'
    assert raw[:raw.index(BLOCK)].count(b'\n') + 1 == 896
    original_lines = raw.splitlines(keepends=True)
    assert len(original_lines) == 1818
    invalid = json.loads(invalid_json.read_text())
    assert invalid['assessment'] == 'INVALID_OR_INCOMPLETE_PARSE'
    assert invalid['running_targets'] == 66 and len(invalid['parse_issues']) == 1
    assert invalid['log_sha256'] == ORIGINAL_SHA256
    result = reconstruct(raw)
    derived_lines = result.splitlines(keepends=True)
    assert len(derived_lines) == 1817
    # Verify every derived byte against its explicit original-line mapping.
    assert derived_lines[:898] == original_lines[:898]
    assert derived_lines[898:900] == original_lines[899:901]
    assert derived_lines[900] == original_lines[898][:-1] + original_lines[901]
    assert derived_lines[901:] == original_lines[902:]
    with derived.open('xb') as output:
        output.write(result)
    command = [sys.executable, str(counter), str(derived), '--result', str(sidecar),
               '--json', str(confirmed_json), '--text', str(confirmed_text)]
    parsed = subprocess.run(command, capture_output=True, text=True, timeout=30)
    assert parsed.returncode == 0, parsed.stderr + parsed.stdout
    assert parsed.stdout == confirmed_text.read_text()
    confirmed = json.loads(confirmed_json.read_text())
    assert confirmed['assessment'] == 'PASSED_RECORDED_RUN'
    assert confirmed['running_targets'] == 67 and confirmed['doctest_targets'] == 5
    assert not confirmed['parse_issues'] and not confirmed['cargo_errors']
    assert confirmed['totals'] == invalid['totals'] == {
        'passed': 1287, 'failed': 0, 'ignored': 5, 'measured': 0, 'filtered_out': 0}
    for prefix, selected in [
        ('unittests src/main.rs (/cache/target/debug/deps/alia_server-', 0),
        ('tests/appraisal_provenance.rs ', 20),
    ]:
        target, = [item for item in confirmed['targets'] if item['target'].startswith(prefix)]
        assert target['outer_harness_start']['selected'] == selected
        assert target['counted_summary']['passed'] == selected
        assert not target['nested_harness_starts'] and not target['excluded_earlier_summaries']
    after = {str(path.relative_to(ROOT)): sha(path.read_bytes()) for path in inputs}
    assert before == after, 'original evidence or parser changed'
    report = {
        'kind': 'derived evidence; not an original command log and not a test rerun',
        'scope': 'one exact reviewed stderr Cargo heading split around the preceding stdout zero-test summary',
        'inputs_sha256_before': before, 'inputs_sha256_after': after,
        'original_log_lines': 1818, 'derived_log_lines': 1817,
        'original_block_lines': [896, 904],
        'original_block_utf8': BLOCK.decode(),
        'transformation': 'Move line 899 prefix after line 901, remove only its LF, and concatenate with line 902. All other line bytes are copied unchanged.',
        'line_mapping_1_based_inclusive': [
            {'derived': [1, 898], 'original': [1, 898], 'operation': 'verbatim'},
            {'derived': [899, 900], 'original': [900, 901], 'operation': 'verbatim'},
            {'derived': [901, 901], 'original': [899, 902],
             'operation': 'concatenate original line 899 without final LF, then original line 902; original lines 900/901 are mapped separately above'},
            {'derived': [902, 1817], 'original': [903, 1818], 'operation': 'verbatim'},
        ],
        'self_test': self_tests,
        'parser_command': command, 'parser_exit_code': parsed.returncode,
        'outputs_sha256': {str(path.relative_to(ROOT)): sha(path.read_bytes())
                           for path in [derived, confirmed_json, confirmed_text]},
        'confirmed_assessment': confirmed['assessment'],
        'confirmed_running_targets': 67, 'confirmed_doctest_targets': 5,
        'confirmed_totals': confirmed['totals'],
        'limitations': ['No generalized log repair; other byte shapes or originals fail closed.',
                        'Confirmed report line numbers refer to the derived view; use the mapping for original evidence.',
                        'The original INVALID_OR_INCOMPLETE_PARSE reports remain unchanged.'],
    }
    with report_path.open('x') as output:
        output.write(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'self_test': self_tests, 'assessment': confirmed['assessment'],
                      'targets': '67 Running + 5 Doc-tests', 'totals': confirmed['totals'],
                      'original_evidence_unchanged': before == after,
                      'reconstruction': str(report_path)}))


if __name__ == '__main__':
    main()
