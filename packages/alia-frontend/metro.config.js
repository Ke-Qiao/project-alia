const { getDefaultConfig } = require('expo/metro-config');
const path = require('path');
const { withUniwindConfig } = require('uniwind/metro');

const projectRoot = __dirname;
const workspaceRoot = path.resolve(projectRoot, '../..');
const config = getDefaultConfig(projectRoot);

// Metro needs the workspace root for pnpm's symlinked node_modules layout.
// Expo Router's route root is pinned in app.json so it still reads this
// package's app/ directory when Metro's projectRoot is widened.
config.projectRoot = workspaceRoot;
config.watchFolders = [projectRoot];
config.resolver.nodeModulesPaths = [
  path.resolve(projectRoot, 'node_modules'),
  path.resolve(workspaceRoot, 'node_modules'),
];

module.exports = withUniwindConfig(config, {
  // relative path to your global.css file (from previous step)
  cssEntryFile: './global.css',
  // (optional) path where we gonna auto-generate typings
  // defaults to project's root
  dtsFile: './uniwind-types.d.ts',
});
