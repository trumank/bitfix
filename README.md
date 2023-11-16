# bitfix
Simple Lua-scriptable runtime binary patcher

## building
```shell
$ cargo build --release
```

## usage
Copy `target/release/bitfix.dll` next to the exe to be patched and name it something that will get loaded, such as `d3d9.dll` or `x3daudio1_7.dll`.
Lua patches will be loaded from a `bitfix/` directory.


## examples

Increasing `MaxAttackers` to 200 in DRG:
```lua
-- bitfix/max_attackers.lua
return {
  {
    --- UPlayerAttackPositionComponent::GetScore
    pattern = '48 89 5C 24 08 48 89 6C 24 10 48 89 74 24 18 57 48 83 EC 30 48 8B 01 41 0F',
    match = function(ctx)
      ctx[ctx:address() + 89] = 200
      ctx[ctx:address() + 187] = 200
    end
  },
  {
    --- UAttackerPositioningComponent::UAttackerPositioningComponent
    pattern = '48 89 5C 24 08 48 89 6C 24 10 48 89 74 24 18 48 89 7C 24 20 41 56 48 81 EC D0 00 00 00 48 8B F9 E8 ?? ?? ?? ?? 48 8B D0 48 8B CF E8 ?? ?? ?? ?? 33 DB',
    match = function(ctx)
      ctx[ctx:address() + 56] = 200
    end
  }
}
```

Allow sticky flames to stick to any actor, not just terrain:
```lua
-- bitfix/stickier_flame.lua
return {
  {
    --- UStickyFlameSpawner::TrySpawnStickyFlameHit
    pattern = '48 89 5c 24 08 48 89 6c 24 10 48 89 74 24 18 57 48 83 ec 70 48 8b f9 48 8b f2 48 8d 4a 68',
    match = function(ctx)
      ctx[ctx:address() + 0x2C] = 0xeb
      ctx[ctx:address() + 0x2D] = 0x32
    end
  },
}
```
