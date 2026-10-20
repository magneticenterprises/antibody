// Shared TypeScript code generation for the API contract.
//
// Reads the authoritative OpenAPI document (generated from Rust) and renders
// `src/api/schema.d.ts` with openapi-typescript. openapi-typescript's default
// lint config rejects duplicate operationIds, so a contract regression there
// fails generation (and the drift check) loudly instead of collapsing types.
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import openapiTS, { astToString, COMMENT_HEADER } from 'openapi-typescript';

export const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const specPath = resolve(root, '../../openapi/openapi.json');
export const outputPath = resolve(root, 'src/api/schema.d.ts');

export async function generate() {
  const spec = JSON.parse(readFileSync(specPath, 'utf8'));
  const ast = await openapiTS(spec, { silent: true });
  return `${COMMENT_HEADER}${astToString(ast)}`;
}
