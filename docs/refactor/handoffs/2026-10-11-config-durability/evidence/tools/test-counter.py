from pathlib import Path
import importlib.util,tempfile,json
r=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('counter',r/'count_cargo_tests.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
def summary(n):return f'test result: ok. {n} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n'
a='Running tests/a.rs (a)\nrunning 3 tests\n'+summary(3)
with tempfile.TemporaryDirectory() as d:
 p=Path(d)/'log'
 p.write_text(a+'\nrunning 11 tests\n\nRunning tests/b.rs (b)\n'+summary(11));v=m.parse_log(p);assert not v['parse_issues'];assert v['totals']['passed']==14;assert len(v['adjacent_pre_heading_starts'])==1
 p.write_text('Running tests/a.rs (a)\nrunning 3 tests\nrunning 1 test\n'+summary(1)+summary(3)+'Running tests/b.rs (b)\nrunning 11 tests\n'+summary(11));v=m.parse_log(p);assert not v['parse_issues'];assert v['totals']['passed']==14;assert v['excluded_nested_summary_count']==1;assert not v['adjacent_pre_heading_starts']
 p.write_text(a+'running 11 tests\nintervening output\nRunning tests/b.rs (b)\n'+summary(11));v=m.parse_log(p);assert v['parse_issues'];assert not v['adjacent_pre_heading_starts']
 p.write_text(a+'running 11 tests\nRunning tests/b.rs (b)\n'+summary(12));v=m.parse_log(p);assert v['parse_issues']
with tempfile.TemporaryDirectory() as d:
 p=Path(d)/'log'
 prefix='Running tests/a.rs (a)\nrunning 3 tests\nRunning tests/b.rs (b)\n'
 p.write_text(prefix+'test previous_tail ... ok\n'+summary(3)+'running 2 tests\n'+summary(2));v=m.parse_log(p);assert not v['parse_issues'];assert v['totals']['passed']==5;assert len(v['delayed_previous_target_tails'])==1
 p.write_text(prefix+'running 2 tests\n'+summary(2));v=m.parse_log(p);assert v['parse_issues'];assert not v['delayed_previous_target_tails']
 p.write_text(prefix+summary(4)+'running 2 tests\n'+summary(2));v=m.parse_log(p);assert v['parse_issues'];assert not v['delayed_previous_target_tails']
 p.write_text(prefix+'unrelated diagnostic\n'+summary(3)+'running 2 tests\n'+summary(2));v=m.parse_log(p);assert v['parse_issues'];assert not v['delayed_previous_target_tails']
record={'cases':8,'passed':8,'scope':'adjacent early start and delayed matching prior tail accepted; nested child preserved; starts, diagnostics and count mismatches rejected'}
(r/'counter-self-test.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
