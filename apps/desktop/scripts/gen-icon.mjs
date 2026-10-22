// App icon generator: node scripts/gen-icon.mjs app-icon.svg pad   (pad = macOS grid, full = Windows/tray)
// Render to PNG (Chromium) then: npx tauri icon <png>. Curve shared with src/lib/comet.ts (MARK).
// generates icon SVG: tapered comet trail along a parabola
import { writeFileSync } from "node:fs";
const P0=[250,700], P1=[512,40], P2=[774,700]; // quadratic -> apex y=370
const TH=0.57;
const B=(t)=>[(1-t)**2*P0[0]+2*(1-t)*t*P1[0]+t*t*P2[0],(1-t)**2*P0[1]+2*(1-t)*t*P1[1]+t*t*P2[1]];
const D=(t)=>{const x=2*(1-t)*(P1[0]-P0[0])+2*t*(P2[0]-P1[0]);const y=2*(1-t)*(P1[1]-P0[1])+2*t*(P2[1]-P1[1]);const l=Math.hypot(x,y);return [x/l,y/l];};
export function build({ pad=true, size=1024 } = {}) {
const N=60, L=[], R=[];
for(let i=0;i<=N;i++){const t=0.04+(TH-0.04)*i/N;const [x,y]=B(t);const [dx,dy]=D(t);const u=i/N;const w=2+44*Math.pow(u,1.6);L.push([x-dy*w,y+dx*w]);R.push([x+dy*w,y-dx*w]);}
const f=(p)=>p.map(([x,y])=>`${x.toFixed(1)} ${y.toFixed(1)}`).join(" L");
const trail=`M${f(L)} L${f(R.reverse())} Z`;
const [hx,hy]=B(TH); const [tx,ty]=D(TH); const ang=Math.atan2(ty,tx)*180/Math.PI;
let dots="";
for(let i=1;i<=7;i++){const t=TH+(1-TH)*(i/7.6);const [x,y]=B(t);dots+=`<circle cx="${x.toFixed(1)}" cy="${y.toFixed(1)}" r="${(11-i*0.6).toFixed(1)}" fill="#CFF8FE" opacity="${(0.7-i*0.06).toFixed(2)}"/>`;}
const [ex,ey]=P2, [sx,sy]=P0;
const o = pad ? {x:100,w:824,rx:186} : {x:24,w:976,rx:220};
return `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 1024 1024">
<defs>
<linearGradient id="bg" x1="0" y1="0" x2="0.3" y2="1"><stop offset="0" stop-color="#161A2B"/><stop offset="1" stop-color="#05060A"/></linearGradient>
<radialGradient id="g1" cx="0.2" cy="0.95" r="0.8"><stop offset="0" stop-color="#22C9DE" stop-opacity="0.55"/><stop offset="0.5" stop-color="#22C9DE" stop-opacity="0.07"/><stop offset="1" stop-color="#22C9DE" stop-opacity="0"/></radialGradient>
<radialGradient id="g2" cx="0.95" cy="0.02" r="0.75"><stop offset="0" stop-color="#7B5CF2" stop-opacity="0.6"/><stop offset="0.6" stop-color="#7B5CF2" stop-opacity="0.05"/><stop offset="1" stop-color="#7B5CF2" stop-opacity="0"/></radialGradient>
<linearGradient id="trail" gradientUnits="userSpaceOnUse" x1="${sx}" y1="${sy}" x2="${hx}" y2="${hy}"><stop offset="0" stop-color="#2FB9CC" stop-opacity="0"/><stop offset="0.35" stop-color="#3CCFE2" stop-opacity="0.5"/><stop offset="0.8" stop-color="#8EEFFB" stop-opacity="0.95"/><stop offset="1" stop-color="#F2FEFF"/></linearGradient>
<linearGradient id="head" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#FFFFFF"/><stop offset="1" stop-color="#A7F1FB"/></linearGradient>
<linearGradient id="rim" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#FFF" stop-opacity="0.24"/><stop offset="0.5" stop-color="#FFF" stop-opacity="0.04"/><stop offset="1" stop-color="#FFF" stop-opacity="0.12"/></linearGradient>
<filter id="glow" x="-60%" y="-60%" width="220%" height="220%"><feGaussianBlur stdDeviation="24"/></filter>
<filter id="glowE" x="-100%" y="-400%" width="300%" height="900%"><feGaussianBlur stdDeviation="20"/></filter>
<filter id="soft" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="12"/></filter>
<filter id="grain"><feTurbulence type="fractalNoise" baseFrequency="0.85" numOctaves="2" stitchTiles="stitch"/><feColorMatrix values="0 0 0 0 1  0 0 0 0 1  0 0 0 0 1  0 0 0 0.06 0"/></filter>
<clipPath id="sq"><rect x="${o.x}" y="${o.x}" width="${o.w}" height="${o.w}" rx="${o.rx}"/></clipPath>
</defs>
${pad ? `<rect x="${o.x}" y="${o.x+16}" width="${o.w}" height="${o.w}" rx="${o.rx}" fill="#000" opacity="0.32" filter="url(#soft)"/>` : ""}
<g clip-path="url(#sq)">
<rect x="0" y="0" width="1024" height="1024" fill="url(#bg)"/>
<rect x="0" y="0" width="1024" height="1024" fill="url(#g1)"/>
<rect x="0" y="0" width="1024" height="1024" fill="url(#g2)"/>
<rect x="0" y="0" width="1024" height="1024" filter="url(#grain)"/>
<ellipse cx="${ex}" cy="${ey+8}" rx="70" ry="16" fill="#48D6E8" opacity="0.4" filter="url(#glowE)"/>
${dots}
<circle cx="${ex}" cy="${ey}" r="30" fill="none" stroke="#E9FDFF" stroke-width="12" opacity="0.92"/>
<circle cx="${sx}" cy="${sy}" r="24" fill="#48D6E8" opacity="0.85"/>
<path d="${trail}" fill="#48D6E8" opacity="0.45" filter="url(#glow)"/>
<path d="${trail}" fill="url(#trail)"/>
<g transform="translate(${hx.toFixed(1)} ${hy.toFixed(1)}) rotate(${ang.toFixed(1)})">
<rect x="-90" y="-90" width="180" height="180" rx="50" fill="#5FE0F0" opacity="0.75" filter="url(#glow)"/>
<rect x="-72" y="-72" width="144" height="144" rx="42" fill="url(#head)"/>
<rect x="-60" y="-66" width="120" height="40" rx="20" fill="#FFF" opacity="0.55"/>
</g>
<rect x="${o.x+1.5}" y="${o.x+1.5}" width="${o.w-3}" height="${o.w-3}" rx="${o.rx-1.5}" fill="none" stroke="url(#rim)" stroke-width="3"/>
</g></svg>`;
}
const [,, out="icon.svg", mode="pad"] = process.argv;
if (out !== "--lib") writeFileSync(out, build({ pad: mode === "pad" }));
// export path data for in-app mark

