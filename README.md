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

Stop the drop pod from eating flares. (why does it do this?)
```lua
-- bitfix/non_flare_devouring_drop_pod.lua
return {
  {
    pattern = '3B 51 ?? 7F ?? 48 8B 49 ?? 48 39 04 D1 75 ?? 48 8B D3 48 8B CF E8 ?? ?? ?? ?? 48 8B 5C 24',
    match = function(ctx)
      ctx[ctx:address() + 13] = 0xEB
    end
  },
}
```

Fix game crashing if more than 8 players are in the lobby during mission load
```lua
-- bitfix/increased_players_fix.lua
return {
  {
    pattern = '48 8b c4 48 89 48 08 55 57 48 8d a8 58 ff ff ff 48 81 ec 98 01 00 00 48 83 79 30 00 48 8b f9 0f 84',
    match = function(ctx)
      ctx[ctx:address()] = 0xC3
    end
  }
}
```

Allow scaling beyond hard coded limit of 4 players in a lobby.
NOTE: Make sure all player count based arrays in difficulty settings have enough
values for the number of players or bad things will happen!
```lua
-- bitfix/increased_players_difficulty_scaling_fix.lua
local patch = function(ctx)
  ctx[ctx:address() + 1] = 0xff
end
return {
  { match = patch, pattern = 'ba 04 00 00 00 3b c2 0f 4e d0' },
  { match = patch, pattern = 'b9 04 00 00 00 3b c1 0f 4e c8' },
  { match = patch, pattern = 'b9 04 00 00 00 8b 80 ?? ?? 00 00 3b c1 0f 4d c1' },
  { match = patch, pattern = 'b9 04 00 00 00 8b 40 08 3b c1 0f 4d c1' },
}
```

Show normal terrain on terrain scanner instead of scanner material.
```lua
-- bitfix/normal_terrain_scanner_mat.lua
return {
  {
    pattern = '74 ?? 49 8B 4C 24 ?? EB ?? 49 8B 0C 24',
    match = function(ctx)
      ctx[ctx:address()] = 0x75
    end
  }
}
```
