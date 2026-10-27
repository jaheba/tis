'use strict';
const data=JSON.parse(document.getElementById('ir-data').textContent);
const svg=document.getElementById('graph'),details=document.getElementById('details');
const ns='http://www.w3.org/2000/svg';
const nodes=new Map(data.nodes.map(node=>[node.id,node]));
for(const edge of data.edges){if(!nodes.has(edge.to))nodes.set(edge.to,{id:edge.to,header:`missing b${edge.to}`,parameters:[],instructions:[],terminator:'<missing block>',terminated:false});}
const definitions=new Map();
for(const node of nodes.values()){for(const id of node.parameters)definitions.set(id,{node});for(const instruction of node.instructions)definitions.set(instruction.id,{node,instruction});}
function element(name,attributes={},parent=svg,text){const value=document.createElementNS(ns,name);for(const [key,item] of Object.entries(attributes))value.setAttribute(key,item);if(text!==undefined)value.textContent=text;parent.appendChild(value);return value;}
function html(name,text,parent=details){const value=document.createElement(name);if(text!==undefined)value.textContent=text;parent.appendChild(value);return value;}
const defs=element('defs');const marker=element('marker',{id:'arrow',viewBox:'0 0 10 10',refX:9,refY:5,markerWidth:7,markerHeight:7,orient:'auto-start-reverse'},defs);element('path',{d:'M 0 0 L 10 5 L 0 10 z',fill:'#8b99ad'},marker);
const depth=new Map();if(nodes.has(0))depth.set(0,0);const queue=nodes.has(0)?[0]:[];
for(let index=0;index<queue.length;index++){const id=queue[index];for(const edge of data.edges.filter(edge=>edge.from===id)){if(!depth.has(edge.to)){depth.set(edge.to,depth.get(id)+1);queue.push(edge.to);}}}
let last=Math.max(0,...depth.values());
for(const node of nodes.values()){node.reachable=depth.has(node.id);if(!node.reachable)depth.set(node.id,++last);}
const levels=new Map();for(const node of nodes.values()){const level=depth.get(node.id);if(!levels.has(level))levels.set(level,[]);levels.get(level).push(node);}
const positions=new Map();
for(const node of nodes.values()) {
  const lines=[node.header,node.terminator,...node.instructions.map(i=>i.text)];
  node.width=Math.max(280,Math.min(600,Math.max(...lines.map(line=>line.length))*7.3+34));
}
const levelWidth=level=>level.reduce((sum,node)=>sum+node.width,0)+Math.max(0,level.length-1)*72;
const width=Math.max(760,...[...levels.values()].map(level=>levelWidth(level)+80));
let top=40;
for(const [,level] of [...levels.entries()].sort((a,b)=>a[0]-b[0])) {
  let height=0,x=(width-levelWidth(level))/2;
  for(const node of level) {
    const box={x,y:top,w:node.width,h:91+node.instructions.length*24};
    positions.set(node.id,box);
    height=Math.max(height,box.h);
    x+=node.width+72;
  }
  top+=height+104;
}
const edgeLayer=element('g'),nodeLayer=element('g');
for(const edge of data.edges){const a=positions.get(edge.from),b=positions.get(edge.to);if(!a||!b)continue;let path,labelX,labelY;
if(depth.get(edge.to)>depth.get(edge.from)){const x1=a.x+a.w/2,y1=a.y+a.h,x2=b.x+b.w/2,y2=b.y;path=`M${x1},${y1} C${x1},${(y1+y2)/2} ${x2},${(y1+y2)/2} ${x2},${y2}`;labelX=(x1+x2)/2;labelY=(y1+y2)/2-8;}
else{const x1=a.x+a.w,y1=a.y+a.h*(edge.from===edge.to?.75:.5),x2=b.x+b.w,y2=b.y+b.h*(edge.from===edge.to?.25:.5),bend=Math.max(x1,x2)+65;path=`M${x1},${y1} C${bend},${y1} ${bend},${y2} ${x2},${y2}`;labelX=bend-10;labelY=(y1+y2)/2;}
element('path',{d:path,class:`edge ${edge.kind}`,'marker-end':'url(#arrow)'},edgeLayer);element('text',{x:labelX,y:labelY,class:'edge-label','text-anchor':'middle'},edgeLayer,edge.label);}
const trim=(text,width)=>{const limit=Math.floor((width-32)/7.3);return text.length>limit?text.slice(0,limit-1)+'…':text;};
for(const node of nodes.values()){const p=positions.get(node.id),specialized=node.instructions.some(i=>i.feedback?.strategy==='Integer');const group=element('g',{class:`block${specialized?' specialized':''}${node.reachable?'':' unreachable'}${node.terminated?'':' unterminated'}`,'data-block':node.id,transform:`translate(${p.x},${p.y})`,tabindex:0,role:'button','aria-label':node.header},nodeLayer);element('rect',{width:p.w,height:p.h},group);element('text',{x:16,y:26,class:'block-title'},group,trim(node.header,p.w));element('line',{x1:0,y1:40,x2:p.w,y2:40,class:'separator'},group);
node.instructions.forEach((instruction,index)=>{const row=element('g',{class:'instruction','data-value':instruction.id,tabindex:0,role:'button','aria-label':instruction.text},group);element('rect',{x:7,y:48+index*24,width:p.w-14,height:23},row);element('text',{x:16,y:64+index*24,class:instruction.feedback?.strategy==='Integer'?'specialized-label':''},row,trim(instruction.text,p.w));row.addEventListener('click',event=>{event.stopPropagation();selectValue(instruction.id);});row.addEventListener('keydown',event=>{if(event.key==='Enter'||event.key===' '){event.stopPropagation();selectValue(instruction.id);}});});
element('text',{x:16,y:p.h-16,class:'terminator'},group,trim(node.terminator,p.w));group.addEventListener('click',()=>selectBlock(node.id));group.addEventListener('keydown',event=>{if(event.key==='Enter'||event.key===' ')selectBlock(node.id);});}
function clearSelection(){for(const item of svg.querySelectorAll('.selected,.definition,.use'))item.classList.remove('selected','definition','use');details.replaceChildren();}
function selectBlock(id){clearSelection();const node=nodes.get(id);svg.querySelector(`[data-block="${id}"]`)?.classList.add('selected');document.getElementById('selection').textContent=node.header;const code=html('pre',node.instructions.map(i=>i.text).concat(node.terminator).join('\n'));code.className='code';html('h3','Block parameters');const box=html('div');box.className='operands';for(const value of node.parameters){const button=html('button',`%${value}`,box);button.className='pill';button.onclick=()=>selectValue(value);}if(!node.parameters.length)html('p','No block parameters.').className='hint';if(!node.reachable)html('p','This block is unreachable from b0.').className='hint';}
function selectValue(id){clearSelection();const found=definitions.get(id);document.getElementById('selection').textContent=`Value %${id}`;if(!found){html('p','No definition found.').className='hint';return;}svg.querySelector(`[data-block="${found.node.id}"]`)?.classList.add('selected');for(const node of nodes.values())for(const instruction of node.instructions){const row=svg.querySelector(`[data-value="${instruction.id}"]`);if(instruction.id===id)row?.classList.add('definition');if(instruction.inputs.includes(id))row?.classList.add('use');}
const instruction=found.instruction;html('pre',instruction?instruction.text:`Block parameter of ${found.node.header}`).className='code';if(instruction){html('h3','Inputs');const box=html('div');box.className='operands';for(const value of instruction.inputs){const button=html('button',`%${value}`,box);button.className='pill';button.onclick=()=>selectValue(value);}if(!instruction.inputs.length)html('p','No input values.').className='hint';if(instruction.feedback){html('h3','Runtime feedback');const stats=html('div');stats.className='stats';for(const [name,value] of [['Strategy',instruction.feedback.strategy],['Executions',instruction.feedback.executions],['Integer fast paths',instruction.feedback.fast_paths],['Guard failures',instruction.feedback.guard_failures]]){html('span',name,stats);html('b',String(value),stats);}html('p','Integer specialization checks both operand types and overflow. A failed guard uses generic addition.').className='hint';}}
html('h3','Users');let uses=0;for(const node of nodes.values())for(const user of node.instructions)if(user.inputs.includes(id)){const button=html('button',`%${user.id} in b${node.id}`);button.className='pill';button.onclick=()=>selectValue(user.id);uses++;}for(const edge of data.edges){const target=nodes.get(edge.to);if(target?.parameters.includes(id))html('p',`Incoming edge from b${edge.from}: ${edge.label}`).className='hint';}if(!uses)html('p','No instruction users; the value may be used by a terminator or edge.').className='hint';}
let view=[0,0,width+100,top];function apply(){svg.setAttribute('viewBox',view.join(' '));}function fit(){view=[0,0,width+100,top];apply();}function zoom(factor,x=.5,y=.5){const w=view[2]*factor,h=view[3]*factor;if(w<150||w>100000)return;view=[view[0]+(view[2]-w)*x,view[1]+(view[3]-h)*y,w,h];apply();}
document.getElementById('fit').onclick=fit;
document.getElementById('zoom-in').onclick=()=>zoom(.8);
document.getElementById('zoom-out').onclick=()=>zoom(1.25);
svg.addEventListener('wheel',event=>{
  event.preventDefault();
  const matrix=svg.getScreenCTM();
  if(!matrix)return;
  const point=new DOMPoint(event.clientX,event.clientY).matrixTransform(matrix.inverse());
  zoom(event.deltaY>0?1.12:.89,(point.x-view[0])/view[2],(point.y-view[1])/view[3]);
},{passive:false});
let drag=null;
svg.addEventListener('pointerdown',event=>{
  if(event.target.closest('[data-block]'))return;
  const matrix=svg.getScreenCTM();
  if(!matrix)return;
  drag={x:event.clientX,y:event.clientY,view:[...view],scale:1/matrix.a};
  svg.setPointerCapture(event.pointerId);
});
svg.addEventListener('pointermove',event=>{
  if(!drag)return;
  view=[drag.view[0]-(event.clientX-drag.x)*drag.scale,drag.view[1]-(event.clientY-drag.y)*drag.scale,drag.view[2],drag.view[3]];
  apply();
});
svg.addEventListener('pointerup',()=>drag=null);
svg.addEventListener('pointercancel',()=>drag=null);
document.getElementById('download').onclick=()=>{const url=URL.createObjectURL(new Blob([data.dot],{type:'text/vnd.graphviz'}));const link=document.createElement('a');link.href=url;link.download='ir.dot';link.click();setTimeout(()=>URL.revokeObjectURL(url),1000);};
const count=data.nodes.reduce((n,node)=>n+node.instructions.length,0),specialized=data.nodes.flatMap(n=>n.instructions).filter(i=>i.feedback?.strategy==='Integer').length;document.getElementById('summary').textContent=`${data.nodes.length} blocks · ${count} instructions · ${specialized} specialized additions`;fit();if(nodes.has(0))selectBlock(0);
