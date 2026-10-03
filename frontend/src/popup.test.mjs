import test from 'node:test';
import assert from 'node:assert/strict';
import {loginDestination, popupLogin, watchPopup} from './popup.ts';

test('IAM popup consumes only its exact origin, window, message shape and one-use attempt', async () => {
  const target = new EventTarget();
  let poll, timeout, completions = 0, failures = [];
  globalThis.location = {origin:'https://starter.example',pathname:'/new',search:'',hash:''};
  globalThis.window = {addEventListener:target.addEventListener.bind(target),removeEventListener:target.removeEventListener.bind(target),setInterval:fn=>(poll=fn,1),setTimeout:fn=>(timeout=fn,2),clearInterval(){},clearTimeout(){}};
  const popup = {closed:false,close(){this.closed=true;},location:{replace(){}}};
  const pending = watchPopup(popup,'starter:login',async()=>{completions++;},e=>failures.push(e));
  pending.navigate('attempt-1','https://iam.example/login');
  const body = {type:'starter:login',attempt_id:'attempt-1',status:'complete'};
  const dispatch = (data=body,origin=location.origin,source=popup) => {
    const event = new Event('message'); Object.assign(event,{data,origin,source});target.dispatchEvent(event);
  };
  dispatch(body,'https://attacker.example'); dispatch(body,location.origin,{});
  dispatch({...body,attempt_id:'another'}); dispatch({...body,type:'starter:briefcase'});
  dispatch({...body,status:'unexpected'}); dispatch({...body,error:{token:'not-a-message'}});
  assert.equal(completions,0);assert.equal(popup.closed,false);
  dispatch({...body,status:'error'});dispatch({...body,status:'error'});
  assert.equal(failures.length,1);assert.equal(popup.closed,false);
  dispatch(); dispatch(); assert.equal(completions,1);assert.equal(popup.closed,true);assert.equal(failures.length,1);
  const cancelled = {...popup,closed:false};
  watchPopup(cancelled,'starter:briefcase',async()=>{completions++;},e=>failures.push(e));
  cancelled.closed=true;poll();timeout();assert.equal(failures.length,2);assert.equal(completions,1);
});

test('login destination stays on the configured IAM origin and safe login route',()=>{
  const attempt = {attempt_id:'opaque',iam_origin:'https://iam.example',login_url:'https://iam.example/login?identity_kind=carbon'};
  assert.equal(loginDestination(attempt),attempt.login_url);
  for(const change of [{login_url:'https://attacker.example/login'},{login_url:'https://secret@iam.example/login'},{login_url:'https://iam.example/logout'},{iam_origin:'https://iam.example/path'},{attempt_id:''},{iam_origin:'http://iam.example',login_url:'http://iam.example/login'}]) assert.throws(()=>loginDestination({...attempt,...change}));
});

test('blocked popups keep the chosen identity and original path in full-page login',()=>{
  let destination;
  globalThis.window={open:()=>null};
  globalThis.location={pathname:'/new',search:'?from=registry',hash:'',assign:value=>{destination=value;}};
  popupLogin('silicon','',()=>assert.fail('Blocked popup must use the full-page endpoint'),async()=>{},()=>{});
  const url=new URL(destination,'https://starter.example');
  assert.equal(url.pathname,'/auth/login');assert.equal(url.searchParams.get('identity_kind'),'silicon');
  assert.equal(url.searchParams.get('return_to'),'/new?from=registry');
});
