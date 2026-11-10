const assert = require('node:assert/strict');
const path = require('node:path');
const { createRequire } = require('node:module');
const web = path.resolve(process.argv[2]);
const localRequire = createRequire(path.join(web, 'package.json'));
const { createJiti } = createRequire(localRequire.resolve('eslint'))('jiti');
const jiti = createJiti(__filename, { alias: { '@': web }, jsx: { runtime: 'automatic' } });
const React = localRequire('react');
const { renderToString } = localRequire('react-dom/server');
const data = jiti(path.join(web, 'lib/data.ts'));
const { DataFetchingProvider } = jiti(path.join(web, 'lib/data-provider.tsx'));
const { healthCheck } = jiti(path.join(web, 'lib/api/generated/sdk.gen.ts'));
const { cn } = jiti(path.join(web, 'lib/utils.ts'));
const expected = process.env.NEXT_PUBLIC_ALIA_API_URL || 'http://127.0.0.1:3000';
let requests = 0;
global.fetch = () => { requests++; throw new Error('The provider check must not call an API'); };
function Probe() {
  assert.equal(data.useClient().getConfig().baseUrl, expected);
  data.useData(healthCheck, {});
  return React.createElement('span', null, 'hooks connected');
}
assert.match(renderToString(React.createElement(DataFetchingProvider, null, React.createElement(Probe))), /hooks connected/);
assert.equal(requests, 0);
for (const key of ['useData', 'useDataImmutable', 'useInfinite', 'useMutation', 'usePreload', 'useClient']) assert.equal(typeof data[key], 'function');
assert.equal(cn('p-2', false && 'hidden', 'p-4'), 'p-4');
console.log(JSON.stringify({ baseUrl: expected, sharedProviderHooks: true, networkRequests: requests, cnConflictMerge: true }));
