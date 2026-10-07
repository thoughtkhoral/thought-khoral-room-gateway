// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import Ajv2020 from 'ajv/dist/2020.js';
import addFormats from 'ajv-formats';
import { parseStrictJson, canonicalBytes, semanticErrors } from './conversation-validation.mjs';

const loadText = path => readFile(new URL(`../${path}`, import.meta.url), 'utf8');
const load = async path => JSON.parse(await loadText(path));
const ajv = new Ajv2020({ allErrors: true, strict: true });
addFormats(ajv);
const validators = new Map();
for (const name of ['turn', 'view', 'catalog', 'task', 'input', 'update', 'result', 'error', 'ack']) {
  ajv.addSchema(await load(`schemas/agent-conversation-v1/${name}.schema.json`));
}
for (const name of ['turn', 'view', 'catalog', 'task', 'input', 'update', 'result', 'error', 'ack']) {
  validators.set(name, ajv.getSchema(`https://github.com/thoughtkhoral/thought-khoral-contracts/schemas/agent-conversation-v1/${name}.schema.json`));
}
ajv.addSchema(await load('schemas/envelope.schema.json'));
validators.set('retained-event', ajv.compile(await load('schemas/room-event.schema.json')));
const prefix = 'fixtures/agent-conversation-v1/';
const cases = await load(prefix + 'manifest.json');
const seen = new Set();
for (const fixture of cases) {
  assert.ok(/^(valid|invalid)\/[a-z0-9-]+\.json$/.test(fixture.path), 'fixture path must be local');
  assert.ok(!seen.has(fixture.path), `duplicate fixture ${fixture.path}`);
  seen.add(fixture.path);
  let errors = [], value;
  try { value = parseStrictJson(await loadText(prefix + fixture.path)); }
  catch (error) { errors.push(error.message); }
  if (!errors.length && fixture.schema === 'canonical') {
    const bytes = canonicalBytes(value.value);
    assert.equal(bytes.toString('utf8'), value.canonical, fixture.path);
    assert.equal(createHash('sha256').update(bytes).digest('hex'), value.digest, fixture.path);
  } else if (!errors.length) {
    const validate = validators.get(fixture.schema);
    assert.ok(validate, `unknown named schema ${fixture.schema}`);
    if (!validate(value)) errors.push('schema: ' + ajv.errorsText(validate.errors));
    else errors.push(...semanticErrors(fixture.schema, value, fixture.context ?? {}));
  }
  assert.equal(errors.length === 0, fixture.valid, `${fixture.path}: ${errors.join('; ') || 'unexpectedly accepted'}`);
  if (!fixture.valid) {
    assert.ok(errors.some(error => error.startsWith(fixture.reason)), `${fixture.path}: must fail for ${fixture.reason}, got ${errors.join('; ')}`);
  }
}
for (const directory of ['valid', 'invalid']) {
  const files = await readdir(new URL(`../${prefix}${directory}/`, import.meta.url));
  for (const file of files) assert.ok(seen.has(`${directory}/${file}`), `unlisted fixture ${directory}/${file}`);
}
assert.ok(cases.some(f => f.valid) && cases.some(f => !f.valid), 'both fixture groups are required');
console.log(`validated ${cases.length} conversation schema, semantic, canonical and retained compatibility fixtures`);
