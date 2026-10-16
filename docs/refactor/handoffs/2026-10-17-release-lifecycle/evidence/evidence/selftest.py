import importlib.util,json,time
from pathlib import Path
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('lifecycle',Path(__file__).with_name('lifecycle.py'));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class Process:
 def __init__(self,codes):self.codes=iter(codes)
 def poll(self):return next(self.codes)
def raises(kind,fn):
 try:fn()
 except kind:return
 raise AssertionError('expected '+kind.__name__)
with patch.object(m.time,'monotonic',return_value=10):
 assert m.remaining(14,7)==4
 raises(TimeoutError,lambda:m.remaining(10,7))
with patch.object(m.time,'monotonic',return_value=0):
 assert m.wait_ready(lambda _:True,Process([None,None]),30) is True
 raises(RuntimeError,lambda:m.wait_ready(lambda _:True,Process([0]),30))
 raises(RuntimeError,lambda:m.wait_ready(lambda _:True,Process([None,0]),30))
with patch.object(m.time,'monotonic',side_effect=[0,31,31]):
 raises(TimeoutError,lambda:m.wait_ready(lambda _:True,Process([None]),30))
print(json.dumps({'checks':6,'passed':6,'scope':'deadline cap/expiry, protocol success, premature/during-readiness exit, late success rejected; no app processes started'}))
