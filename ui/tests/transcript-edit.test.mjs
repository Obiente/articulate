import test from "node:test";
import assert from "node:assert/strict";
import { replaceSpelling, suggestSpelling } from "../src/transcript-edit.js";

test("repeated multiword spelling corrections preserve surrounding prose", () => {
  const before = "Start with a kit. Considering a kit is a sample project.";
  const after = "Start with AcmeKit. Considering AcmeKit is a sample project.";
  assert.deepEqual(suggestSpelling(before, after), { heard: "a kit", wanted: "AcmeKit" });
  assert.equal(replaceSpelling(before, "a kit", "AcmeKit"), after);
});
test("replacement respects Unicode boundaries and literal punctuation", () => {
  assert.equal(replaceSpelling("a kit, a kitchen, éa kit", "a kit", "AcmeKit"), "AcmeKit, a kitchen, éa kit");
  assert.equal(replaceSpelling("C++ and C++", "C++", "$&"), "$& and $&");
  assert.equal(replaceSpelling("hello", "", "x"), "hello");
  assert.equal(suggestSpelling("Please review this next week.", "Thanks, I'll finish tomorrow."), null);
});
