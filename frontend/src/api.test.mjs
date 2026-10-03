import test from 'node:test';
import assert from 'node:assert/strict';
import {createApi,sessionOf,contextsOf} from './api.ts';
import {consentOf,completionKey} from './consent.ts';
const session={authenticated:true,context_id:'account-a',org_id:'tos',actor:{type:'carbon',public_id:'c:alice'}};
const reply=value=>new Response(JSON.stringify(value),{status:200});

test('ordinary sessions require one canonical account and organization',()=>{
 assert.equal(sessionOf(session).org_id,'tos');
 for(const invalid of [{...session,org_id:null},{...session,context_id:''},{...session,org_ids:['tos','other']},{...session,actor:{type:'silicon',public_id:'c:alice'}}])assert.throws(()=>sessionOf(invalid),/renewed/);
 assert.deepEqual(sessionOf({authenticated:false}),{authenticated:false});
 assert.throws(()=>contextsOf({contexts:[{...session,org_id:'OTHER'}]}));
});
test('API pins header to rendered account and refuses metadata from another cookie',async()=>{
 const client=createApi('https://starter.example');client.establish(session);
 let sent;
 globalThis.fetch=async(_url,init)=>{sent=init.headers.get('x-starter-context');return reply({...session,context_id:'account-b'});};
 await assert.rejects(client.request('/auth/session'),e=>e.status===409);
 assert.equal(sent,'account-a');assert.equal(client.context(),'account-a');
 assert.throws(()=>client.establish({...session,context_id:'account-b'}));
});
test('a slow body cannot populate a newly established context',async()=>{
 const client=createApi('');let finish;
 globalThis.fetch=async()=>({status:200,ok:true,text:()=>new Promise(resolve=>finish=resolve)});
 const pending=client.request('/api/v1/starters');
 await new Promise(resolve=>setTimeout(resolve,0));
 client.establish(session);finish(JSON.stringify([{id:'old.private'}]));
 await assert.rejects(pending,e=>e.status===409);
});
test('204 and structured permission errors retain their semantics',async()=>{
 const client=createApi('');client.establish(session);
 globalThis.fetch=async()=>new Response(null,{status:204});assert.equal(await client.request('/auth/context'),undefined);
 globalThis.fetch=async()=>new Response(JSON.stringify({error:{code:'consent_changed',message:'Review again'}}),{status:412});
 await assert.rejects(client.request('/api/v1/briefcase/authorization'),e=>e.status===412&&e.message==='Review again');
});
const consent={request_id:'request-a',context_id:session.context_id,completed:false,authorization:{id:'iam-a',app_id:'starter',actor:session.actor,org_id:'tos',status:'pending',version:1,expires_at:'2099-01-01T00:00:00Z',authorization_url:'https://iam.example/review',redirect_uri:'https://starter.example/callback',state:'a'.repeat(43)}};
test('feature review binds requester, context, state and IAM request identity',()=>{
 assert.equal(consentOf(consent,session).request_id,'request-a');
 for(const invalid of [{...consent,context_id:'other'},{...consent,authorization:{...consent.authorization,actor:{type:'carbon',public_id:'c:bobby'}}},{...consent,authorization:{...consent.authorization,state:'short'}}])assert.throws(()=>consentOf(invalid,session));
 assert.throws(()=>consentOf({...consent,authorization:{...consent.authorization,id:'iam-other'}},session,consent));
 assert.throws(()=>consentOf({...consent,authorization:{...consent.authorization,authorization_url:'http://iam.example/review'}},session));
 assert.throws(()=>consentOf({...consent,completed:'true'},session));
});
test('manual reviews do not invent callback state, and declines remain displayable',()=>{
 const authorization={...consent.authorization,status:'declined'};delete authorization.redirect_uri;delete authorization.state;
 assert.equal(consentOf({...consent,authorization},session).authorization.status,'declined');
});
test('lost completion replies use stable keys scoped to the exact request and code',async()=>{
 const key=await completionKey('a','request','code');assert.equal(key,await completionKey('a','request','code'));
 assert.notEqual(key,await completionKey('b','request','code'));assert.notEqual(key,await completionKey('a','request','other'));
 assert.match(key,/^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/);
});
