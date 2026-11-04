-- Hacking Mod: every remote player is drawn, outlined, even behind geometry.
-- Context shape of render.entity_visibility is a PROPOSAL (see README); every field is nil-checked.
---@param mod ModCapability
return function(mod)
    mod.content.text:override("IDS_XRAY_SERVER_BANNER", "HACKING MOD: everyone sees everyone")

    mod.hooks:wrap("render.entity_visibility", function(base, ctx)
        local res = base(ctx)
        if type(res) ~= "table" then res = {} end
        if type(ctx) ~= "table" or type(ctx.entities) ~= "table" then return res end
        res.entities = type(res.entities) == "table" and res.entities or {}
        for _, e in ipairs(ctx.entities) do
            if type(e) == "table" and e.kind == "player" and not e.is_local and e.id ~= nil then
                res.entities[#res.entities + 1] = {
                    id = e.id,
                    visible = true,
                    outline = true,
                }
            end
        end
        return res
    end)
end
