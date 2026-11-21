# Provider-independent contracts. Unknown core features deliberately fail closed.
let NonEmptyString = std.contract.from_predicate (fun text =>
  std.is_string text && std.string.length text > 0
    && !(std.string.is_match "^\\s*$" text)
) in
let Identifier = std.contract.from_predicate (fun text =>
  std.is_string text && std.string.is_match "^[A-Za-z_][A-Za-z0-9_-]*$" text
) in
# When checking an explicit literal, a nested reserved record is also data.
# Protect it from nested Value contracts, force their checks, then retain the
# original payload. The protection wrappers never reach the rendered result.
let rec protect = fun item =>
  if std.is_record item then
    if std.record.has_field "__nickel_tf" item then
      { __nickel_tf = { kind = "literal", value = item } }
    else std.record.map (fun _ child => protect child) item
  else if std.is_array item then std.array.map protect item
  else item
in
let protect_children = fun item =>
  if std.is_record item then std.record.map (fun _ child => protect child) item
  else if std.is_array item then std.array.map protect item
  else item
in
let ValueContract = fun Contract => std.contract.custom (fun label item =>
  if std.is_record item && std.record.has_field "__nickel_tf" item then
    let shell = std.contract.apply { __nickel_tf | { kind | String, .. } } label item in
    if shell.__nickel_tf.kind == "expression" then
      std.contract.check { __nickel_tf | { kind | String, source | NonEmptyString } } label item
    else if shell.__nickel_tf.kind == "literal" then
      let checked = std.contract.apply { __nickel_tf | { kind | String, value | Dyn } } label item in
      let payload = std.contract.apply Contract label (protect_children checked.__nickel_tf.value) in
      std.deep_seq payload ('Ok checked)
    else 'Error { message = "Invalid __nickel_tf marker kind; use tf.literal for reserved-shaped data" }
  else std.contract.check Contract label item
) in
let rec Static = std.contract.custom (fun label item =>
  if std.is_record item && std.record.has_field "__nickel_tf" item then
    let checked = std.contract.apply { __nickel_tf | { kind | String, value | Dyn } } label item in
    if checked.__nickel_tf.kind == "literal" then 'Ok checked
    else 'Error { message = "Deferred expressions are not supported in static arguments" }
  else if std.is_record item then
    'Ok (std.record.map (fun _ child => std.contract.apply Static label child) item)
  else if std.is_array item then
    'Ok (std.array.map (std.contract.apply Static label) item)
  else 'Ok item
) in
let IgnoreChanges = std.contract.custom (fun label item =>
  if item == "all" then 'Ok item else std.contract.check (Array String) label item
) in
let Lifecycle = {
  create_before_destroy | Bool | optional,
  prevent_destroy | Bool | optional,
  ignore_changes | IgnoreChanges | optional,
  replace_triggered_by | Array String | optional,
} in
let DataMeta = {
  count | ValueContract std.number.Nat | optional,
  for_each | ValueContract { _ : Dyn } | optional,
  depends_on | Array String | optional,
  provider | String | optional,
  ..
} in
let ResourceMetadata = DataMeta & { lifecycle | Lifecycle | optional, .. } in
let WithMeta = fun Contract Meta names => std.contract.custom (fun label item =>
  if !(std.is_record item) then 'Error { message = "Expected a block object" }
  else
    let meta = std.record.filter (fun name _ => std.array.elem name names) item in
    let body = std.record.filter (fun name _ => !(std.array.elem name names)) item in
    'Ok ((std.contract.apply Contract label body) & (std.contract.apply Meta label meta))
) in
let Instance = fun Contract Meta names => std.contract.custom (fun label item =>
  if std.is_record item && std.record.has_field "count" item && std.record.has_field "for_each" item then
    'Error { message = "count and for_each are mutually exclusive" }
  else std.contract.check (WithMeta Contract Meta names) label item
) in
let OwnedInstance = fun owner inferred Contract Meta names => std.contract.custom (fun label item =>
  if !(std.is_record item) then std.contract.check (Instance Contract Meta names) label item
  else
    let has_provider = std.record.has_field "provider" item in
    let valid_provider = fun address =>
      std.is_string address &&
      (address == owner || std.string.is_match ("^" ++ owner ++ "\\.[A-Za-z_][A-Za-z0-9_-]*$") address)
    in
    if has_provider && !(valid_provider item.provider) then
      'Error { message = "Resource/data provider reference must use its selected schema owner" }
    else
      let configured = if !has_provider && owner != inferred then item & { provider = owner } else item in
      std.contract.check (Instance Contract Meta names) label configured
) in
{
  Value = ValueContract,
  Nullable = fun Contract => std.contract.custom (fun label item =>
    if item == null then 'Ok item else std.contract.check Contract label item
  ),
  NonNull = fun Contract => std.contract.custom (fun label item =>
    if item == null then 'Error { message = "Required values cannot be null" }
    else std.contract.check Contract label item
  ),
  Tuple = fun contracts => std.contract.custom (fun label items =>
    if !(std.is_array items) then 'Error { message = "Expected a tuple array" }
    else if std.array.length items != std.array.length contracts then
      'Error { message = "Tuple has the wrong number of elements" }
    else 'Ok (std.array.generate
      (fun i => std.contract.apply (std.array.at i contracts) label (std.array.at i items))
      (std.array.length items))
  ),
  Bounds = fun minimum maximum Contract => std.contract.custom (fun label items =>
    if !(std.is_array items || std.is_record items) then
      'Error { message = "Nested block collection must be an array or object" }
    else
      let size = if std.is_array items then std.array.length items else std.array.length (std.record.fields items) in
      if size < minimum || (maximum != 0 && size > maximum) then
        'Error { message = "Nested block count is outside schema bounds" }
      else std.contract.check Contract label items
  ),
  # Map-block labels are static HCL labels; their template-looking spellings
  # cannot be distinguished from expression-object keys after JSON export.
  BlockMap = fun Contract => std.contract.custom (fun label items =>
    if !(std.is_record items) then 'Error { message = "Expected map of named blocks" }
    else if std.array.any (std.string.is_match "\\$\\{|%\\{") (std.record.fields items) then
      'Error { message = "Template introducers in nested block labels are unsupported" }
    else std.contract.check { _ : Contract } label items
  ),
  Resource = fun Contract => Instance Contract ResourceMetadata ["count", "for_each", "depends_on", "provider", "lifecycle"],
  Data = fun Contract => Instance Contract DataMeta ["count", "for_each", "depends_on", "provider"],
  ResourceFor = fun owner inferred Contract => OwnedInstance owner inferred Contract ResourceMetadata ["count", "for_each", "depends_on", "provider", "lifecycle"],
  DataFor = fun owner inferred Contract => OwnedInstance owner inferred Contract DataMeta ["count", "for_each", "depends_on", "provider"],
  ProviderConfigs = fun Contract =>
    let Config = WithMeta Contract { alias | Identifier | optional } ["alias"] in
    std.contract.custom (fun label item =>
      if std.is_array item then std.contract.check (Array Config) label item
      else std.contract.check Config label item
    ),
  ResourceMeta = ResourceMetadata,
  Variable = {
    type | NonEmptyString | optional,
    default | Static | optional,
    description | String | optional,
    sensitive | Bool | optional,
    nullable | Bool | optional,
  },
  Output = {
    value | ValueContract Dyn,
    description | String | optional,
    sensitive | Bool | optional,
    depends_on | Array String | optional,
  },
  Terraform = {
    required_version | String | optional,
    required_providers | { _ : { source | String | optional, version | String | optional } } | optional,
  },
}
