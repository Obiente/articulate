import test from "node:test";
import assert from "node:assert/strict";
import { recentApps, replaceProfile } from "../src/features/dictation/profile-options.js";
test("profiles replace case-insensitively without losing other apps",()=>{
 assert.deepEqual(replaceProfile([{app:"EDITOR.EXE"},{app:"chat.exe"}],{app:"editor.exe",insert:true}),[{app:"chat.exe"},{app:"editor.exe",insert:true}]);
});
test("recent apps omit missing names and duplicates",()=>{
 assert.deepEqual(recentApps({recent_dictation_app:"editor.exe"},[{app:"editor.exe"},{}]),["editor.exe"]);
});

import { prepareAction } from "../src/actions.js";
test("system shortcuts fail before preview or desktop dispatch",()=>{
 assert.throws(()=>prepareAction({type:"settings_patch",hotkey:{win:true,key:"L"}}),/reserved/);
 assert.doesNotThrow(()=>prepareAction({type:"settings_patch",hotkey:{ctrl:true,alt:true,key:"Space"}}));
});
