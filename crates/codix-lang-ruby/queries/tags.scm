; From tree-sitter-ruby 0.23.1, queries/tags.scm.

(
  (comment)* @doc
  .
  [
    (method
      name: (_) @name) @definition.method
    (singleton_method
      name: (_) @name) @definition.method
  ]
  (#strip! @doc "^#\\s*")
  (#select-adjacent! @doc @definition.method)
)
(alias
  name: (_) @name) @definition.method
(setter
  (identifier) @ignore)
(
  (comment)* @doc
  .
  [
    (class
      name: [
        (constant) @name
        (scope_resolution
          name: (_) @name)
      ]) @definition.class
    (singleton_class
      value: [
        (constant) @name
        (scope_resolution
          name: (_) @name)
      ]) @definition.class
  ]
  (#strip! @doc "^#\\s*")
  (#select-adjacent! @doc @definition.class)
)
(
  (module
    name: [
      (constant) @name
      (scope_resolution
        name: (_) @name)
    ]) @definition.module
)
(call method: (identifier) @name) @reference.call
(
  [(identifier) (constant)] @name @reference.call
  (#is-not? local)
  (#not-match? @name "^(lambda|load|require|require_relative|__FILE__|__LINE__)$")
)

; Added for codix. Later patterns win over the ones above for the same node.

(program
  (method
    name: (_) @name) @definition.function)

(method
  name: (identifier) @name
  (#eq? @name "initialize")) @definition.constructor

(assignment
  left: (constant) @name) @definition.constant
