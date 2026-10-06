// Run with the real current AI host tools object; do not substitute a stdio client.
// Launch one fresh owned ui_controls fixture first. Pass code, round, result path,
// and a fresh UUID for invoke. save(report) must persist progress before continuing.
// A failed or unconfirmed mutation requires inspecting the original operation.
async function settle(name,args) {
 let raw=await tools["mcp__pixels__"+name](args), d=raw.structuredContent;
 if(!d)throw Error(name+": no structured result");
 const id=d.operation_ref?.operation_id;
 for(let i=0;["running","pending","submitted"].includes(d.result?.state)&&i<40;i++){
 d=(await tools.mcp__pixels__pab_get_operation({device_code:args.device_code,operation_id:id,wait_ms:1000,wait_until:"complete"})).structuredContent;
 }
 return d;
}
async function runNativeUi(o,save){
 const report={source:"native_pixels_host",device_code:o.code,round:o.round,cases:[],started_at:new Date().toISOString()};
 const check=(v,m)=>{if(!v)throw Error(m);};
 const ui=d=>{check(["completed","failed"].includes(d.result?.state)&&d.result?.data?.snapshot?.ui,"operation did not reach a UI terminal result");return d.result.data.snapshot.ui;};
 async function call(name,args,test){
  const d=await settle(name,{device_code:o.code,...args});
  const c={tool:name,operation_id:d.operation_ref?.operation_id??d.result?.request_id,status:"pending"};
  report.cases.push(c);save(report);
  try{if(test)test(d);c.status="pass";}catch(e){c.status="fail";c.reason=e.message;save(report);throw e;}
  save(report);return d;
 }
 const pre=await settle("pab_file_read",{device_code:o.code,path:o.path});
 check(pre.result?.error?.code==="not_found","result exists or unexpected filesystem error");
 const windows=await call("pab_list_windows",{},d=>check(d.result?.state==="completed","windows unavailable"));
 const own=windows.result.data.snapshot.windows.filter(w=>w.title==="PAB UI acceptance fixture");
 check(own.length===1&&own[0].window_ref,"fixture missing/ambiguous/unavailable");
 const scope={type:"window",window_ref:own[0].window_ref};
 const tree=ui(await call("pab_ui_query",{scope},d=>check(!ui(d).truncated,"fixture query incomplete")));
 const elements=tree.elements;
 const ref=name=>{const found=elements.filter(e=>e.name===name);check(found.length===1,"fixture target ambiguous/missing");return found[0].element_ref;};
 report.fixture_window=scope.window_ref;report.fixture_input=ref("Fixture input");save(report);
 check(elements.filter(e=>e.name==="Fixture duplicate").length===2,"duplicate elements lost");
 check(elements.every(e=>e.value===null),"query leaked value");
 await call("pab_ui_get",{element_ref:ref("Fixture secure"),include_value:true},d=>check(ui(d).elements[0].protected&&ui(d).elements[0].value===null,"secure value not redacted"));
 for(const name of ["Fixture readonly","Fixture secure","Fixture disabled"]){
  await call("pab_ui_action",{element_ref:ref(name),action:name==="Fixture disabled"?{type:"invoke"}:{type:"set_value",value:"must-not-write"}},d=>check(ui(d).action_dispatched===false&&!!ui(d).error_code,"protected action not rejected"));
 }
 await call("pab_ui_action",{element_ref:ref("Fixture input"),action:{type:"set_value",value:"must-not-write"},expected:{enabled:false}},d=>check(ui(d).action_dispatched===false&&!!ui(d).error_code,"precondition not enforced"));
 for(const value of ["","PAB 中文🙂"]){
  await call("pab_ui_action",{element_ref:ref("Fixture input"),action:{type:"set_value",value}},d=>check(ui(d).verification==="matched","value verification failed"));
 }
 await call("pab_ui_action",{element_ref:ref("Fixture option"),action:{type:"set_checked",checked:true}},d=>check(ui(d).verification==="matched","checkbox failed"));
 await call("pab_ui_action",{element_ref:ref("Fixture option"),action:{type:"set_checked",checked:true}},d=>check(ui(d).verification==="matched"&&ui(d).action_dispatched===false,"checkbox not idempotent"));
 await call("pab_ui_action",{element_ref:ref("Fixture radio"),action:{type:"select"}},d=>check(ui(d).verification==="matched","radio failed"));
 let selected=elements.find(e=>e.name==="Fixture second"&&e.supported_actions.includes("select"))?.element_ref;
 if(!selected){
  for(const e of elements.filter(e=>["text","text_field"].includes(e.role)&&!e.name&&!e.protected)){
   const v=ui(await call("pab_ui_get",{element_ref:e.element_ref,include_value:true}));
   if(v.elements[0].value==="Fixture second"){selected=e.parent_ref;break;}
  }
 }
 check(selected,"list target missing");
 await call("pab_ui_action",{element_ref:selected,action:{type:"select"}},d=>check(ui(d).verification==="matched","list selection failed"));
 await call("pab_ui_wait",{scope:{type:"element",element_ref:ref("Fixture input")},condition:{type:"value_equals",value:"PAB 中文🙂"},timeout_ms:2000},d=>check(ui(d).outcome==="matched","wait value failed"));
 await call("pab_ui_wait",{scope,selector:{name:"missing fixture element"},condition:{type:"exists"},timeout_ms:500,poll_ms:100},d=>check(ui(d).outcome==="timed_out","wait timeout failed"));
 await call("pab_ui_wait",{scope,selector:{name:"Fixture duplicate"},condition:{type:"enabled",value:true},timeout_ms:500},d=>check(ui(d).error_code==="ambiguous","ambiguous condition accepted"));
 const args={element_ref:ref("Apply fixture"),action:{type:"invoke"},request_id:o.invoke};
 const first=await call("pab_ui_action",args,d=>check(ui(d).action_dispatched===true,"invoke not dispatched"));
 const replay=await call("pab_ui_action",args,d=>check(JSON.stringify(d.result)===JSON.stringify(first.result),"duplicate result changed"));
 let observed=false;
 for(let i=0;i<30;i++){
  const read=await settle("pab_file_read",{device_code:o.code,path:o.path});
  if(read.text){const actual=JSON.parse(read.text.replace(/^\uFEFF/,""));check(actual.clicks===1&&actual.value==="PAB 中文🙂"&&!!actual.checked&&!!actual.radio&&actual.selected===1,"actual application results differ");observed=true;break;}
  await new Promise(r=>setTimeout(r,100));
 }
 check(observed,"application result missing; do not replay invoke");
 await call("pab_ui_get",{element_ref:ref("Fixture input"),include_value:true},d=>check(ui(d).elements[0].value==="PAB 中文🙂","get value mismatch"));
 report.actual_application_verified=true;report.status="pass";report.finished_at=new Date().toISOString();save(report);return report;
}
