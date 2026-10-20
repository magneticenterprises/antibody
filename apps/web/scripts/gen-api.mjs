#!/usr/bin/env node
// Regenerate src/api/schema.d.ts from ../../openapi/openapi.json.
import { writeFileSync } from 'node:fs';
import { generate, outputPath, specPath } from './api-codegen.mjs';

const output = await generate();
writeFileSync(outputPath, output, 'utf8');
console.log(`gen:api: ${specPath} -> ${outputPath}`);
