import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";

const projectRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const loadJson = async (relativePath) =>
  JSON.parse(await readFile(resolve(projectRoot, relativePath), "utf8"));

const rpcSchema = await loadJson("schemas/rpc.schema.json");
const envelopeSchema = await loadJson("schemas/envelope.schema.json");
const roomEventSchema = await loadJson("schemas/room-event.schema.json");
const ajv = new Ajv2020({ allErrors: true, strict: true });
addFormats(ajv);
ajv.addSchema(envelopeSchema);
ajv.addSchema(roomEventSchema);
const validate = ajv.compile(rpcSchema);

const browserAuthentication = {
  jsonrpc: "2.0",
  id: "authenticate-1",
  method: "session.authenticate",
  params: { accessToken: "header.payload.signature" },
};
assert.equal(
  validate(browserAuthentication),
  true,
  `session.authenticate must validate: ${ajv.errorsText(validate.errors)}`,
);

const assertFixtures = async (directory, expectedValid) => {
  const fileNames = (await readdir(resolve(projectRoot, directory))).sort();
  assert.ok(fileNames.length > 0, `${directory} must contain fixtures`);

  for (const fileName of fileNames) {
    const isValid = validate(await loadJson(`${directory}/${fileName}`));
    assert.equal(
      isValid,
      expectedValid,
      `${directory}/${fileName}: ${ajv.errorsText(validate.errors)}`,
    );
  }
};

await assertFixtures("fixtures/valid", true);
await assertFixtures("fixtures/invalid", false);

console.log(
  "validated session authentication and all valid fixtures; rejected all invalid fixtures",
);
