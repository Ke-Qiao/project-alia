import { defineConfig } from '@hey-api/openapi-ts';

export default defineConfig({
  input: './openapi/openapi.json',
  output: 'lib/api/generated',
  plugins: [
    {
      name: '@hey-api/client-fetch',
      baseUrl: false,
      throwOnError: true,
      includeInEntry: true
    },
    {
      name: '@hey-api/sdk',
      responseStyle: 'fields'
    }
  ]
});
