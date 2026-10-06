// Run inside the tool host with its real tools object, not a substitute MCP client.
// Pass a fresh UUID, verified device/OS context and a dedicated absolute temp root.
// Returns only nonsecret observations/IDs. No lifecycle actions or user-file changes.
async function runNativeBasic(tools, options) {
  const {device_code, os, run_id, parent} = options;
  if (!/^\d{9}$/.test(device_code) || !/^[a-f0-9-]{36}$/.test(run_id)) throw Error('invalid run identity');
  if (!['windows','macos'].includes(os)) throw Error('unsupported fixture OS');
  if ((os === 'windows' && parent !== 'C:\\Windows\\Temp') || (os === 'macos' && parent !== '/private/tmp')) throw Error('fixture parent must be approved temp root');
  const sep = os === 'windows' ? '\\' : '/';
  const root = parent + sep + 'pab-acceptance-' + run_id;
  const path = name => root + sep + name;
  const report = {schema_version:1, run_id, suite:'live-basic', source:'native_pixels_host', device_code, os,
    started_at:new Date().toISOString(), cases:[], uncleaned_resources:[]};
  let created = false, uncertain = false;
  let sequence = 0;
  const identified = new Set(['pab_mkdir','pab_file_write','pab_file_patch','pab_file_copy','pab_file_move','pab_file_delete','pab_archive_create','pab_archive_extract','pab_file_hash']);
  const check = (ok, why) => { if (!ok) throw Error(why); };
  async function invoke(name, args) {
    const request_id = identified.has(name) ? run_id.slice(0,24) + String(++sequence).padStart(12,'0') : undefined;
    let raw;
    try { raw = await tools['mcp__pixels__'+name]({...args, device_code, ...(request_id ? {request_id} : {})}); }
    catch (_) { uncertain=true; const error=Error('Transport result unconfirmed; inspect original request'); error.operation_id=request_id; throw error; }
    let data = raw.structuredContent;
    if (!data) throw Error(name + ': missing structured result');
    const reference = data.operation_ref;
    const state = d => d.result?.state ?? d.operation?.state ?? d.task?.state ?? d.snapshot?.state;
    for (let attempt = 0; reference && ['running','pending','submitted','connecting'].includes(state(data)) && attempt < 30; attempt++) {
      try { data = (await tools.mcp__pixels__pab_get_operation({device_code,operation_id:reference.operation_id,wait_until:'complete',wait_ms:1000})).structuredContent; }
      catch (_) { uncertain=true; const error=Error('Poll interrupted; inspect original operation'); error.operation_id=reference.operation_id; throw error; }
    }
    if (reference && ['running','pending','unconfirmed','submitted'].includes(state(data))) {
      uncertain = true;
      const error = Error('result unconfirmed; query original operation');
      error.operation_id = reference.operation_id;
      throw error;
    }
    return {data, reference, error: raw.isError || !!data.error || ['failed','cancelled'].includes(state(data))};
  }
  async function caseRun(name, tool, args, assertion, expectedError=false) {
    const entry = {case:name, tool, started_at:new Date().toISOString(), status:'unconfirmed'};
    report.cases.push(entry);
    try {
      const response = await invoke(tool,args);
      if (response.reference) entry.operation_ids = [response.reference.operation_id];
      check(!!response.error === expectedError, expectedError ? 'expected rejection was not observed' : 'tool returned an error');
      if (assertion) assertion(response.data);
      entry.status='pass'; entry.evidence=expectedError ? 'Expected rejection observed' : 'Terminal result and case assertion verified';
      return response.data;
    } catch (error) {
      entry.status=uncertain ? 'unconfirmed' : 'fail';
      // Only our diagnostic message, never raw payload/arguments.
      entry.evidence=error.operation_id ? 'Query original operation before replay' : String(error.message).slice(0,200);
      if (error.operation_id) entry.operation_ids=[error.operation_id];
      throw error;
    } finally { entry.finished_at=new Date().toISOString(); }
  }
  try {
    await caseRun('create isolated fixture','pab_mkdir',{path:root,parents:false,exist_ok:false}, d=>check(d.result.state==='completed','mkdir not completed'));
    created=true;
    const original='alpha 中文\nbeta\n';
    const write=await caseRun('Unicode path and UTF-8 write','pab_file_write',{path:path('中文 sample.txt'),content:original,overwrite:false}, d=>check(d.result.metadata.size===18,'UTF-8 byte size mismatch'));
    const hash=write.result.metadata.sha256;
    check(/^[a-f0-9]{64}$/.test(hash),'missing content hash');
    await caseRun('read exact Unicode content','pab_file_read',{path:path('中文 sample.txt')}, d=>check(d.text===original,'content differs'));
    await caseRun('reject overwrite conflict','pab_file_write',{path:path('中文 sample.txt'),content:'must-not-replace',overwrite:false},d=>check(d.result.error.code==='already_exists','wrong conflict error'),true);
    await caseRun('reject stale hash','pab_file_patch',{path:path('中文 sample.txt'),expected_hash:'0'.repeat(64),edits:[{find:'alpha',replace:'changed'}]},d=>check(d.result.error.code==='version_conflict','wrong stale hash error'),true);
    await caseRun('dry-run patch','pab_file_patch',{path:path('中文 sample.txt'),expected_hash:hash,dry_run:true,edits:[{find:'alpha',replace:'gamma'}]},d=>check(d.result.state==='completed','dry-run failed'));
    await caseRun('dry-run preserves file','pab_file_read',{path:path('中文 sample.txt')},d=>check(d.text===original,'dry-run modified file'));
    await caseRun('apply hash guarded patch','pab_file_patch',{path:path('中文 sample.txt'),expected_hash:hash,edits:[{find:'alpha',replace:'gamma'}]},d=>check(d.result.state==='completed','patch incomplete'));
    await caseRun('read tail after patch','pab_file_read',{path:path('中文 sample.txt'),mode:'tail',tail_bytes:128},d=>check(d.text==='gamma 中文\nbeta\n','patched content differs'));
    const changed=await caseRun('hash modified file','pab_file_hash',{path:path('中文 sample.txt')},d=>check(d.result.metadata.sha256!==hash,'patch did not change hash'));
    await caseRun('regex content search','pab_file_search',{path:root,query:'gamma.*',regex:true,mode:'content',context_lines:1,max_results:10},d=>check(JSON.stringify(d.result).includes('gamma'),'missing regex match'));
    await caseRun('copy file','pab_file_copy',{path:path('中文 sample.txt'),destination:path('copy.txt'),overwrite:false},d=>check(d.result.state==='completed','copy incomplete'));
    await caseRun('copied hash','pab_file_hash',{path:path('copy.txt')},d=>check(d.result.metadata.sha256===changed.result.metadata.sha256,'copy hash differs'));
    await caseRun('move file','pab_file_move',{path:path('copy.txt'),destination:path('moved.txt'),overwrite:false},d=>check(d.result.state==='completed','move incomplete'));
    await caseRun('moved content','pab_file_read',{path:path('moved.txt')},d=>check(d.text==='gamma 中文\nbeta\n','moved content differs'));
    await caseRun('old move path absent','pab_file_stat',{path:path('copy.txt')},null,true);
    await caseRun('archive create','pab_archive_create',{path:path('fixture.zip'),sources:[path('moved.txt')],overwrite:false},d=>check(d.result.state==='completed','archive incomplete'));
    await caseRun('archive extract','pab_archive_extract',{path:path('fixture.zip'),destination:path('extracted'),overwrite:false},d=>check(d.result.state==='completed','extract incomplete'));
    await caseRun('archive content verified','pab_file_read',{path:path('extracted'+sep+'moved.txt')},d=>check(d.text==='gamma 中文\nbeta\n','archive content differs'));
    await caseRun('empty file round trip','pab_file_write',{path:path('empty'),content:'',overwrite:false},d=>check(d.result.metadata.size===0,'empty file is not empty'));
    for (const [tool,args] of [['pab_system_info',{include_gpu:false,sample_cpu:false}],['pab_list_disks',{limit:20}],
      ['pab_list_processes',{limit:100,sample_cpu:false}],['pab_list_network_interfaces',{limit:20}],
      ['pab_list_sessions',{}],['pab_list_services',{limit:20}]]) {
      await caseRun(tool,tool,args,d=>check(d.result.state==='completed' && !!d.result.data,'no completed system snapshot'));
    }
  } catch (_) {
    // Preserve failed evidence and continue to scoped cleanup, never replay the failed case.
  } finally {
    if (created && !uncertain) {
      try { await caseRun('cleanup owned fixture','pab_file_delete',{path:root,recursive:true,max_entries:200,max_bytes:10485760,max_depth:10},d=>check(d.result.state==='completed','cleanup incomplete')); }
      catch (_) { report.uncleaned_resources.push(root); }
    } else if (created || uncertain) report.uncleaned_resources.push(root);
    report.finished_at=new Date().toISOString();
  }
  return report;
}
