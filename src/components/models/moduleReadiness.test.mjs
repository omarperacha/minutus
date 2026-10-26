import assert from 'node:assert/strict';
import test from 'node:test';
import {moduleSetupReadiness, moduleStatusLabel} from './moduleReadiness.ts';
const module = (required, registrationState = 'notInstalled', source = null) => ({required,registrationState,source});
test('Qwen primary alone cannot finish when its projector is required', () => {
 assert.deepEqual(moduleSetupReadiness([module(true,'available'),module(true)]), {requiredReady:false,allReady:false});
 assert.deepEqual(moduleSetupReadiness([module(true,'available'),module(true,'available')]), {requiredReady:true,allReady:true});
});
test('an optional draft does not block completion after primary and projector are ready', () => {
 assert.deepEqual(moduleSetupReadiness([module(true,'available'),module(true,'available'),module(false)]), {requiredReady:true,allReady:false});
 assert.equal(moduleSetupReadiness([]).requiredReady,false);
});
test('changed or inaccessible required files revoke readiness', () => {
 for(const state of ['unavailable','verificationFailed','incomplete']) assert.equal(moduleSetupReadiness([module(true,state)]).requiredReady,false);
});
test('sources remain distinct after verification', () => {
 assert.equal(moduleStatusLabel(module(true,'available','imported')),'Imported · Verified');
 assert.equal(moduleStatusLabel(module(true,'available','downloaded')),'Downloaded · Verified');
});
