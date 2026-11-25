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

-- Matches a query against the shown name and, for a place, its other name (CK-423: both names are searched whichever is shown). A row is
-- the entry's own fields plus, when only the other name matched, a `label` that adds that name in parentheses. The row's identity
-- (`kind`, `id`) and its sort key (the folded shown name) never depend on the label.
function model.search(entries, query)
  local needle = model.fold(query:match("^%s*(.-)%s*$"))
  if needle == "" then return {}, 0 end
  local found = {}
  for _, entry in ipairs(entries) do
    local name = model.fold(entry.name)
    local by_name = name:find(needle, 1, true) ~= nil
    local by_other = entry.other ~= nil and model.fold(entry.other):find(needle, 1, true) ~= nil
    if by_name or by_other then
      local row = {}
      for key, field in pairs(entry) do row[key] = field end
      if not by_name then row.label = entry.name .. " (" .. entry.other .. ")" end
      found[#found + 1] = {entry = row, name = name}
    end
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
