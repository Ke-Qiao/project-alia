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
    // Generated client, and coss ui sources copied in verbatim by `pnpm ui:add`.
    customGlobs: ['lib/api/generated/**', 'components/ui/**', 'hooks/use-media-query.ts']
  }
});
