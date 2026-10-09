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
record={'cases':4,'passed':4,'scope':'adjacent early start accepted; nested child preserved; intervening output and count mismatch rejected'}
(r/'counter-self-test.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
