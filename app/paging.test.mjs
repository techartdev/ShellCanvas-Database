import test from "node:test";
import assert from "node:assert/strict";
import { nextOffset } from "./paging.js";
test("variable-size pages advance without skipping rows",()=>{assert.equal(nextOffset({offset:100,rows:[1,2,3]}),103)});
