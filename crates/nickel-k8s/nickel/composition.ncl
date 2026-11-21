# Reusable, typed compositions. Named components retain Kubernetes identities.
{
  define = fun Input build => fun parameters =>
    let checked = parameters | Input in
    std.deep_seq checked (build checked),
  compose = fun selected_target components => {
    target = selected_target,
    resources = std.array.flatten (std.record.values components),
  },
}
