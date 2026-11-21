{
 define = fun Input build => fun parameters =>
   let checked = parameters | Input in std.deep_seq checked (build checked),
 opentofu = fun selected_target selected_schema body => {backend="opentofu", target=selected_target, schema=selected_schema, configuration=body},
 kubernetes = fun selected_target body => {backend="kubernetes",target=selected_target,configuration=body},
 binding = fun source name destination gate => {fromUnit=source,output=name,to=destination,condition=gate},
}
