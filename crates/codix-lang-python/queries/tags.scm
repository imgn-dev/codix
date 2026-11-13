; From tree-sitter-python 0.25.0, queries/tags.scm.

(module (expression_statement (assignment left: (identifier) @name) @definition.constant))

(class_definition
  name: (identifier) @name) @definition.class

(function_definition
  name: (identifier) @name) @definition.function

(call
  function: [
      (identifier) @name
      (attribute
        attribute: (identifier) @name)
  ]) @reference.call

; Added for codix. Later patterns win over the ones above for the same node.

; Module-level names that are not upper case are variables.
(module
  (expression_statement
    (assignment
      left: (identifier) @name
      (#not-match? @name "^[A-Z][A-Z0-9_]*$")) @definition.variable))

; Functions defined in a class body are methods; `__init__` constructs.
(class_definition
  body: (block
    [
      (function_definition
        name: (identifier) @name) @definition.method
      (decorated_definition
        definition: (function_definition
          name: (identifier) @name) @definition.method)
    ]))

(class_definition
  body: (block
    [
      (function_definition
        name: (identifier) @name
        (#eq? @name "__init__")) @definition.constructor
      (decorated_definition
        definition: (function_definition
          name: (identifier) @name
          (#eq? @name "__init__")) @definition.constructor)
    ]))

; Class-level assignments are fields.
(class_definition
  body: (block
    (expression_statement
      (assignment
        left: (identifier) @name) @definition.field)))

; Added for codix: supertypes, as written.

(class_definition
  superclasses: (argument_list [(identifier) (attribute)] @supertype)) @supertypes
