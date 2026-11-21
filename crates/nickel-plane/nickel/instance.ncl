{
 apiVersion | String,
 id | String,
 composition | {name | String, version | String},
 displayName | String | optional,
 parameters | Dyn | optional,
 policy | { mode | String | optional, .. } | optional,
 units | { _ | {
   backend | String, target | String, schema | String | optional,
   configuration | Dyn,
   dependsOn | Array Dyn | optional,
   bindings | Array Dyn | optional,
   outputs | { _ | {pointer | String, sensitive | Bool | optional} } | optional,
 } },
}
