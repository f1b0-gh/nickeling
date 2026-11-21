let collision = fun message => (message | std.contract.from_predicate (fun _ => false))
in
let merge_unique = fun left right =>
  let duplicate = std.array.any (fun key => std.record.has_field key left) (std.record.fields right) in
  if duplicate then collision "Duplicate composition identity" else left & right
in
let merge_nested = fun left right =>
  std.array.fold_left (fun result name =>
    let incoming = std.record.get name right in
    let combined = if std.record.has_field name result then
      merge_unique (std.record.get name result) incoming else incoming in
    (if std.record.has_field name result then std.record.remove name result else result) & {"%{name}" = combined}
  ) left (std.record.fields right)
in
let merge_component = fun left right =>
  std.array.fold_left (fun result block =>
    let incoming = std.record.get block right in
    let combined = if std.record.has_field block result then
      if block == "resource" || block == "data" then merge_nested (std.record.get block result) incoming
      else merge_unique (std.record.get block result) incoming
    else incoming in
    (if std.record.has_field block result then std.record.remove block result else result) & {"%{block}" = combined}
  ) left (std.record.fields right)
in
{
  define = fun Input build => fun parameters =>
    let checked = parameters | Input in
    std.deep_seq checked (build checked),
  compose = fun components => std.array.fold_left merge_component {} (std.record.values components),
}
