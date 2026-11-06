; Written for codix: scopes, local definitions and references of Python.
; General patterns come first: for a node captured as a reference by
; several patterns, the one written last wins.

; Scopes
;-------

; A name assigned anywhere in a scope is that scope's for the whole scope,
; so every scope is hoisted. A class body is not visible from its methods.
(function_definition) @local.scope.function
(lambda) @local.scope.function
(class_definition body: (block) @local.scope.class)
(list_comprehension) @local.scope.comprehension
(set_comprehension) @local.scope.comprehension
(dictionary_comprehension) @local.scope.comprehension
(generator_expression) @local.scope.comprehension

; Definitions
;------------

(parameters (identifier) @local.definition.parameter)
(lambda_parameters (identifier) @local.definition.parameter)
(default_parameter name: (identifier) @local.definition.parameter)
(typed_parameter (identifier) @local.definition.parameter)
(typed_default_parameter name: (identifier) @local.definition.parameter)
(list_splat_pattern (identifier) @local.definition.parameter)
(dictionary_splat_pattern (identifier) @local.definition.parameter)

; Assigning a name again in its scope writes the same variable.
((assignment left: (identifier) @local.definition.variable)
  (#set! local.reuse))
((assignment left: (pattern_list (identifier) @local.definition.variable))
  (#set! local.reuse))
((assignment left: (tuple_pattern (identifier) @local.definition.variable))
  (#set! local.reuse))
((for_statement left: (identifier) @local.definition.variable)
  (#set! local.reuse))
((for_statement left: (pattern_list (identifier) @local.definition.variable))
  (#set! local.reuse))
((for_statement left: (tuple_pattern (identifier) @local.definition.variable))
  (#set! local.reuse))
((for_in_clause left: (identifier) @local.definition.variable)
  (#set! local.reuse))
((for_in_clause left: (pattern_list (identifier) @local.definition.variable))
  (#set! local.reuse))
((for_in_clause left: (tuple_pattern (identifier) @local.definition.variable))
  (#set! local.reuse))
((as_pattern alias: (as_pattern_target (identifier) @local.definition.variable))
  (#set! local.reuse))

; `:=` in a comprehension assigns in the enclosing function.
((named_expression name: (identifier) @local.definition.variable)
  (#set! local.scope "function")
  (#set! local.reuse))

(import_statement name: (dotted_name . (identifier) @local.definition.import))
(import_statement name: (aliased_import alias: (identifier) @local.definition.import))
(import_from_statement name: (dotted_name (identifier) @local.definition.import))
(import_from_statement name: (aliased_import alias: (identifier) @local.definition.import))

(global_statement (identifier) @local.global)
(nonlocal_statement (identifier) @local.nonlocal)

; References
;-----------

(identifier) @local.reference.read
(type (identifier) @local.reference.type)

(call function: (identifier) @local.reference.call)
(decorator (identifier) @local.reference.call)
(augmented_assignment left: (identifier) @local.reference.write)

(attribute
  object: (_) @local.receiver
  attribute: (identifier) @local.reference.read)
(call
  function: (attribute
    object: (_) @local.receiver
    attribute: (identifier) @local.reference.call))
(assignment
  left: (attribute
    object: (_) @local.receiver
    attribute: (identifier) @local.reference.write))
(augmented_assignment
  left: (attribute
    object: (_) @local.receiver
    attribute: (identifier) @local.reference.write))

; A keyword argument names a parameter of the callee.
(call
  function: (_) @local.receiver
  arguments: (argument_list
    (keyword_argument name: (identifier) @local.reference.read)))

; Module names are not names in scope.
(import_statement name: (dotted_name (identifier) @local.skip . (identifier)))
(import_statement name: (dotted_name (_) (identifier) @local.skip))
(import_statement name: (aliased_import name: (dotted_name (identifier) @local.skip)))
(import_from_statement module_name: (dotted_name (identifier) @local.skip))
(import_from_statement name: (aliased_import name: (dotted_name (identifier) @local.skip)))
