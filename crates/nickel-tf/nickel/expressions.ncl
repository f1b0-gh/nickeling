let NonEmptyString = std.contract.from_predicate (fun text =>
  std.is_string text && std.string.length text > 0
    && !(std.string.is_match "^\\s*$" text)
) in
let Segment = std.contract.from_predicate (fun text =>
  std.is_string text && std.string.is_match "^[A-Za-z_][A-Za-z0-9_-]*$" text
) in
{
  expr | NonEmptyString -> Dyn
    = fun text => { __nickel_tf = { kind = "expression", source = text } },
  literal = fun payload => { __nickel_tf = { kind = "literal", value = payload } },
  ref | (Array Segment) -> Dyn
    = fun segments =>
      let nonempty = segments | std.array.NonEmpty in
      { __nickel_tf = { kind = "expression", source = std.string.join "." nonempty } },
}
