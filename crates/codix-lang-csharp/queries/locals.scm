; Written for codix: scopes, local definitions and references of C#.
; General patterns come first: for a node captured as a reference by
; several patterns, the one written last wins.

; Scopes
;-------

; Members are visible in the whole body. A type with a base list, or a
; partial one, may have members codix cannot see.
(class_declaration body: (declaration_list) @local.scope.class)
(struct_declaration body: (declaration_list) @local.scope.class)
(record_declaration body: (declaration_list) @local.scope.class)
(interface_declaration body: (declaration_list) @local.scope.class)
(class_declaration (base_list) body: (declaration_list) @local.scope.class
  (#set! local.opaque))
(struct_declaration (base_list) body: (declaration_list) @local.scope.class
  (#set! local.opaque))
(record_declaration (base_list) body: (declaration_list) @local.scope.class
  (#set! local.opaque))
(interface_declaration (base_list) body: (declaration_list) @local.scope.class
  (#set! local.opaque))
((class_declaration (modifier) @partial body: (declaration_list) @local.scope.class)
  (#eq? @partial "partial")
  (#set! local.opaque))

; Type parameters and primary constructor parameters are visible in the
; whole declaration.
(class_declaration) @local.scope.type
(struct_declaration) @local.scope.type
(record_declaration) @local.scope.type
(interface_declaration) @local.scope.type

(method_declaration) @local.scope.function
(constructor_declaration) @local.scope.function
(local_function_statement) @local.scope.function
(lambda_expression) @local.scope.function
(anonymous_method_expression) @local.scope.function

(block) @local.scope.block
(for_statement) @local.scope.block
(foreach_statement) @local.scope.block
(catch_clause) @local.scope.block
(using_statement) @local.scope.block
(switch_section) @local.scope.block

; Definitions
;------------

(parameter name: (identifier) @local.definition.parameter)
(lambda_expression parameters: (implicit_parameter) @local.definition.parameter)
(variable_declarator name: (identifier) @local.definition.variable)
(foreach_statement left: (identifier) @local.definition.variable)
(catch_declaration name: (identifier) @local.definition.variable)
(declaration_expression name: (identifier) @local.definition.variable)
(declaration_pattern name: (identifier) @local.definition.variable)
(type_parameter name: (identifier) @local.definition.type_parameter)
(using_directive name: (identifier) @local.definition.import)

; References
;-----------

(identifier) @local.reference.read

(invocation_expression function: (identifier) @local.reference.call)
(assignment_expression left: (identifier) @local.reference.write)

(member_access_expression
  expression: (_) @local.receiver
  name: (identifier) @local.reference.read)
(invocation_expression
  function: (member_access_expression
    expression: (_) @local.receiver
    name: (identifier) @local.reference.call))
(assignment_expression
  left: (member_access_expression
    expression: (_) @local.receiver
    name: (identifier) @local.reference.write))
(qualified_name
  qualifier: (_) @local.receiver
  name: (identifier) @local.reference.read)

; A named argument names a parameter of the callee.
(invocation_expression
  function: (_) @local.receiver
  arguments: (argument_list (argument name: (identifier) @local.reference.read)))

; Names that are no references: namespaces.
(namespace_declaration name: (identifier) @local.skip)
(namespace_declaration name: (qualified_name (identifier) @local.skip))
(file_scoped_namespace_declaration name: (identifier) @local.skip)
(using_directive (identifier) @local.skip .)
(using_directive (qualified_name (identifier) @local.skip))
