import {test} from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,readFileSync,writeFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {spawnSync} from 'node:child_process';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
test('build audit requires confirmation and successful checkpoint plus recheck',{skip:process.platform!=='win32'},()=>{
 const source=readFileSync(path.join(root,'scripts/bat/build_app.bat'),'utf8');
 assert.ok(source.includes('call :audit_changelog\n')||source.includes('call :audit_changelog\r\n'));
 // Match the label, not the earlier CALL site.
 const label=source.match(/^:audit_changelog\r?\n([\s\S]*?)^:error_exit/m)[0].replace(/:error_exit$/,'');
 for(const scenario of [
  {name:'already valid',initial:0,input:'',expected:0,calls:['check']},
  {name:'decline',input:'n\n',expected:1,calls:['check']},
  {name:'empty answer',input:'\n',expected:1,calls:['check']},
  {name:'end of input',input:'',expected:1,calls:['check']},
  {name:'confirmed',input:'y\n',expected:0,calls:['check','checkpoint','check']},
  {name:'checkpoint fails',input:'y\n',checkpoint:1,expected:1,calls:['check','checkpoint']},
  {name:'recheck fails',input:'y\n',recheck:1,expected:1,calls:['check','checkpoint','check']},
  {name:'CI fails without prompting',input:'y\n',ci:'1',expected:1,calls:['check']},
 ]){
  const dir=mkdtempSync(path.join(tmpdir(),'zoltraak-build-audit-'));
  try{
   writeFileSync(path.join(dir,'fixture.bat'),'@echo off\r\nsetlocal enabledelayedexpansion\r\ncall :audit_changelog\r\nexit /b !ERRORLEVEL!\r\n'+label.replaceAll('node scripts\\changelog.mjs','call node_stub.bat').replace(/\r?\n/g,'\r\n'));
   writeFileSync(path.join(dir,'node_stub.bat'),'@echo off\r\necho %1>>calls.txt\r\nif "%1"=="checkpoint" (\r\n echo done>checkpoint.txt\r\n exit /b %CHECKPOINT%\r\n)\r\nif exist checkpoint.txt exit /b %RECHECK%\r\nexit /b %INITIAL%\r\n');
   const result=spawnSync('cmd.exe',['/d','/c','fixture.bat'],{cwd:dir,encoding:'utf8',input:scenario.input,timeout:5000,env:{...process.env,CI:scenario.ci??'',INITIAL:String(scenario.initial??1),CHECKPOINT:String(scenario.checkpoint??0),RECHECK:String(scenario.recheck??0)}});
   assert.equal(result.status,scenario.expected,scenario.name+': '+result.stdout+result.stderr);
   assert.deepEqual(readFileSync(path.join(dir,'calls.txt'),'utf8').trim().split(/\r?\n/),scenario.calls,scenario.name);
  }finally{rmSync(dir,{recursive:true,force:true});}
 }
});
