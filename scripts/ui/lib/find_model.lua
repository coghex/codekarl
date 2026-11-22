-- Search ordering for the find popup. The scenario uses these accented Latin letters; matching folds them to their base letter.
local model = {}
local folded = {}
for base, variants in pairs({
  a = "áàâäãåÁÀÂÄÃÅ", e = "éèêëÉÈÊË", i = "íìîïÍÌÎÏ",
  o = "óòôöõÓÒÔÖÕ", u = "úùûüÚÙÛÜ", c = "çÇ", n = "ñÑ", y = "ýÿÝ",
}) do
  for _, code in utf8.codes(variants) do folded[utf8.char(code)] = base end
end

function model.fold(value)
  local result = {}
  for _, code in utf8.codes(value) do
    local char = utf8.char(code)
    result[#result + 1] = folded[char] or string.lower(char)
  end
  return table.concat(result)
end

local order = {location = 1, character = 2, river = 3}

function model.search(entries, query)
  local needle = model.fold(query:match("^%s*(.-)%s*$"))
  if needle == "" then return {}, 0 end
  local found = {}
  for _, entry in ipairs(entries) do
    local name = model.fold(entry.name)
    if name:find(needle, 1, true) then found[#found + 1] = {entry = entry, name = name} end
  end
  table.sort(found, function(a, b)
    if a.name ~= b.name then return a.name < b.name end
    if a.entry.kind ~= b.entry.kind then return order[a.entry.kind] < order[b.entry.kind] end
    return a.entry.id < b.entry.id
  end)
  local rows = {}
  for index = 1, math.min(#found, 200) do rows[index] = found[index].entry end
  return rows, math.max(0, #found - 200)
end

-- Reconcile a saved identity against a freshly filtered revision.
function model.index_of(rows, identity)
  if identity == nil then return nil end
  for index, row in ipairs(rows) do
    if row.kind == identity.kind and row.id == identity.id then return index end
  end
  return nil
end

return model
