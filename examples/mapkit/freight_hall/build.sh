#!/bin/bash
# rebuild the whole Freight Hall project from the downloaded pack
# S: a folder with glb/ (the downloaded models) and fh/ (this folder), R: the folder holding the tools/ programs built
S=${S:?set S}; R=${R:?set R}
F=$S/fh; DC=${DC:-dc-mapkit}
B=$F/base; P=$F/project
G=$S/glb; C=(--license CC-BY-4.0 --author NVIDIA --source https://huggingface.co/datasets/nvidia/simready-assets)
if [ ! -d "$B" ]; then
  $DC new "$B" --template flat --name "Freight Hall" --author DeCombat --json >/dev/null 2>&1
  "$DC" asset "$B" "$G/warehouse_h10m_straight_a04.glb" --name wh --textures-only --keep 5,6,8,9,12,15,29,35,36,38,40,43 --tex 512 --relief 0.35 --title warehouse_h10m_straight_a04 "${C[@]}" --json >/dev/null 2>&1
  "$DC" asset "$B" "$G/warehouse_h10m_straight_a04.glb" --name wh --textures-only --keep 34 --tex 1024 --relief 0.35 --title warehouse_h10m_straight_a04 "${C[@]}" --json >/dev/null 2>&1
  "$DC" asset "$B" "$G/structure_wall_straight_a01.glb" --name sw --textures-only --keep 0,1,7 --tex 512 --relief 0.35 --title structure_wall_straight_a01 "${C[@]}" --json >/dev/null 2>&1
  "$DC" asset "$B" "$G/structure_wall_straight_a01.glb" --name sw --textures-only --keep 3 --tex 1024 --relief 0.35 --title structure_wall_straight_a01 "${C[@]}" --json >/dev/null 2>&1
  "$DC" asset "$B" "$G/structure_wall_entrance_a01.glb" --name en --textures-only --keep 8 --tex 512 --relief 0.35 --title structure_wall_entrance_a01 "${C[@]}" --json >/dev/null 2>&1
  rm -f "$B"/assets/ground.png "$B"/assets/wall.png "$B"/assets/block.png "$B"/assets/sky.png
  X=$F/tx/p/assets
  $R/ribs "$B/assets/paint_red.png" $X/en_10.png $X/en_16.png $X/en_19.png "$B/assets/wh_43.png" 0.4 1.7
  $R/ribs "$B/assets/paint_orange.png" $X/en_10.png $X/en_16.png $X/en_19.png "$B/assets/wh_40.png" 0.4 1.7
  $R/ribs "$B/assets/clad.png" $X/en_10.png $X/en_17.png $X/en_19.png $X/en_17.png 0.12 1.4 0.95
  $R/crop "$B/assets/hazard.png" $X/en_0.png 0 270 560 560 512
  $R/spray "$F/art"
  for w in gf_rush:4.4 gf_three:4.4 gf_crew:3.8 gf_cover:3.6 gf_skull:2.4 gf_tag:6 gf_camp:6 gf_up:2.2 gf_high:2.2 gf_marks:2.7 gf_flank:4.4 gf_wrong:3.2 gf_dead:3.2; do
    "$DC" asset "$B" "$F/art/${w%%:*}.png" --name "${w%%:*}" --width "${w##*:}" --own --json >/dev/null 2>&1
  done
  : > "$B/.stamps"
fi
while read n f t x o; do
  [ -n "$n" ] || continue
  grep -qxF "$n $f $t $x $o" "$B/.stamps" && continue
  [ -s "$G/$f.glb" ] || curl -sL --max-time 300 "https://huggingface.co/datasets/codepawl/simready-glb/resolve/main/glb/$f.glb" -o "$G/$f.glb"
  a=(asset "$B" "$G/$f.glb" --name "$n" --max-tris "$t" --tex "$x" --title "$f" "${C[@]}")
  case "$o" in box|round|rebake) a+=(--proxy "$o") ;; rebake:*) a+=(--proxy rebake --leave-out "${o#rebake:}") ;; strict) a+=(--strict) ;; esac
  "$DC" "${a[@]}" --json 2>/dev/null | python3 -I -c "
import json,sys
d=json.load(sys.stdin)
if not d['ok']: print('FAIL $n',d['diagnostics'])
else:
  r=d['result']; print('$n',r['source_tris'],'->',r['tris'],'parts',r['parts'],'png',r['png_bytes'],[round(v*50) for v in r['size_m']])"
  echo "$n $f $t $x $o" >> "$B/.stamps"
done < "$F/assets.txt"
rm -rf "$P"; cp -R "$B" "$P"; rm -f "$P/.stamps"
"$DC" run "$P" "$F/gen.lua" --json 2>/dev/null | python3 -I -c "
import json,sys
d=json.load(sys.stdin); print('run',d['ok'],(d.get('result') or {}).get('prints'))
for x in d['diagnostics'][:8]: print(x['severity'],x['code'],x.get('path'),x['message'][:200])"
"$DC" validate "$P" --json 2>/dev/null | python3 -I -c "
import json,sys
d=json.load(sys.stdin); print('validate',d['ok'])
for x in d['diagnostics']:
  if x['severity']!='info': print(x['severity'],x['code'],x.get('path'),x['message'][:170])"
"$DC" build "$P" --json 2>/dev/null | python3 -I -c "
import json,sys
d=json.load(sys.stdin); print('build',d['ok'],(d.get('result') or {}).get('stats'))
for x in d['diagnostics']:
  if x['severity']!='info': print(x['severity'],x['code'],x.get('path'),x['message'][:200])"
