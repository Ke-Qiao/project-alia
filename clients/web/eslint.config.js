import { sukka } from 'eslint-config-sukka';

export default sukka({
  ts: {
    allowDefaultProject: []
  },
  react: true,
  next: true,
  node: false,
  pnpm: false,
  ignores: {
    customGlobs: ['lib/api/generated/**']
  }
});
