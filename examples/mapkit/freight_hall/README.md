# Freight Hall: the source

A small 3v3 warehouse from an owner's drawing, built with `dc-mapkit`. The whole project is in `project/`, and its built world file is too large to commit (15.6 MB against the 10 MB of
`tools/assetcheck.py`), so the pack is built from it:

```
dc-mapkit export examples/mapkit/freight_hall/project --zip --no-rom-paths
```

That writes `project/build/freight_hall.zip`, the file `community/vanilla_plus.json` names by its hash.

- `gen.lua`: the layout, lights, props, graffiti placement and the collision over painted surfaces.
- `assets.txt`: every SimReady model, as `name file max_tris tex mode`.
- `build.sh`: rebuilds the project from the models, about 20 s in a release build.
- `tools/`: small programs that make the ribbed steel and hazard textures from SimReady pictures (`ribs.rs`, `crop.rs`) and
  draw the graffiti, our own art (`spray.rs`). Each needs only the `image` crate.

Models and textures: [NVIDIA SimReady](https://huggingface.co/datasets/nvidia/simready-assets), CC BY 4.0, listed in the pack's
`CREDITS.txt`. No game files. Bullet marks land on collision, so painted walls and floors carry thin `skin` layers; check by
shooting (docs/MAPKIT_AGENTS.md).
