import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";

const projectRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const loadJson = async (relativePath) =>
  JSON.parse(await readFile(resolve(projectRoot, relativePath), "utf8"));

const packageMetadata = await loadJson("package.json");
const protocol = await readFile(resolve(projectRoot, "protocol.md"), "utf8");
assert.equal(
  packageMetadata.name,
  "@n2n/contracts",
  "the vendored contract package identity must remain stable",
);
assert.match(
  protocol,
  /`n2n\.room\.v1` remains a retained compatibility wire value/,
  "protocol documentation must identify n2n.room.v1 as a retained compatibility wire value",
);

const rpcSchema = await loadJson("schemas/rpc.schema.json");
const envelopeSchema = await loadJson("schemas/envelope.schema.json");
const roomEventSchema = await loadJson("schemas/room-event.schema.json");
const ajv = new Ajv2020({ allErrors: true, strict: true });
addFormats(ajv);
ajv.addSchema(envelopeSchema);
ajv.addSchema(roomEventSchema);
const validate = ajv.compile(rpcSchema);
const validateRoomEvent = ajv.compile(roomEventSchema);

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

const deletedDecisionEvent = {
  contractVersion: "n2n.room.v1",
  requestId: "77777777-7777-4777-8777-777777777777",
  roomId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
  occurredAt: "2026-09-18T12:03:00Z",
  sequence: 7,
  eventId: "88888888-8888-4888-8888-888888888888",
  eventType: "decision.deleted",
  actor: {
    id: "99999999-9999-4999-8999-999999999999",
    role: "human",
  },
  payload: {
    decisionId: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
    priorStatus: "draft",
    title: "Adopt JSON-RPC",
    summary: "Room requests use standardized JSON-RPC 2.0 envelopes.",
    sourceEventIds: [],
  },
};
assert.equal(
  validateRoomEvent(deletedDecisionEvent),
  true,
  `decision.deleted must validate: ${ajv.errorsText(validateRoomEvent.errors)}`,
);

const deletedDecisionEventWithoutSummary = structuredClone(deletedDecisionEvent);
delete deletedDecisionEventWithoutSummary.payload.summary;
assert.equal(
  validateRoomEvent(deletedDecisionEventWithoutSummary),
  false,
  "decision.deleted must require payload.summary",
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

const assertRoomEventFixtures = async (directory, expectedValid) => {
  const fileNames = (await readdir(resolve(projectRoot, directory))).sort();
  assert.ok(fileNames.length > 0, `${directory} must contain fixtures`);

  for (const fileName of fileNames) {
    const isValid = validateRoomEvent(await loadJson(`${directory}/${fileName}`));
    assert.equal(
      isValid,
      expectedValid,
      `${directory}/${fileName}: ${ajv.errorsText(validateRoomEvent.errors)}`,
    );
  }
};

await assertFixtures("fixtures/valid", true);
await assertFixtures("fixtures/invalid", false);
await assertRoomEventFixtures("fixtures/invalid-events", false);

console.log(
  "validated session authentication and all valid fixtures; rejected all invalid fixtures",
);
