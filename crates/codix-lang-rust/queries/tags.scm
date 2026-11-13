; From tree-sitter-rust 0.24.2, queries/tags.scm.


(struct_item
    name: (type_identifier) @name) @definition.class

(enum_item
    name: (type_identifier) @name) @definition.class

(union_item
    name: (type_identifier) @name) @definition.class


(type_item
    name: (type_identifier) @name) @definition.class


(declaration_list
    (function_item
        name: (identifier) @name) @definition.method)


(function_item
    name: (identifier) @name) @definition.function

(trait_item
    name: (type_identifier) @name) @definition.interface

(mod_item
    name: (identifier) @name) @definition.module


(macro_definition
    name: (identifier) @name) @definition.macro


(call_expression
    function: (identifier) @name) @reference.call

(call_expression
    function: (field_expression
        field: (field_identifier) @name)) @reference.call

(macro_invocation
    macro: (identifier) @name) @reference.call


(impl_item
    trait: (type_identifier) @name) @reference.implementation

(impl_item
    type: (type_identifier) @name
    !trait) @reference.implementation

; Added for codix. Later patterns win over the ones above for the same node:
; types get their own kinds, and methods of an impl block are qualified by
; its type.

(struct_item
  name: (type_identifier) @name) @definition.struct

(union_item
  name: (type_identifier) @name) @definition.struct

(enum_item
  name: (type_identifier) @name) @definition.enum

(type_item
  name: (type_identifier) @name) @definition.type_alias

(trait_item
  name: (type_identifier) @name) @definition.trait

(enum_variant
  name: (identifier) @name) @definition.enum_member

(field_declaration
  name: (field_identifier) @name) @definition.field

(const_item
  name: (identifier) @name) @definition.constant

(static_item
  name: (identifier) @name) @definition.variable

(associated_type
  name: (type_identifier) @name) @definition.type_alias

(trait_item
  body: (declaration_list
    [
      (function_item
        name: (identifier) @name) @definition.method
      (function_signature_item
        name: (identifier) @name) @definition.method
    ]))

(impl_item
  type: [
    (type_identifier) @parent
    (generic_type
      type: (type_identifier) @parent)
    (scoped_type_identifier
      name: (type_identifier) @parent)
  ]
  body: (declaration_list
    [
      (function_item
        name: (identifier) @name) @definition.method
      (const_item
        name: (identifier) @name) @definition.constant
      (type_item
        name: (type_identifier) @name) @definition.type_alias
    ]))

; Added for codix: supertraits, as written.

(trait_item
  bounds: (trait_bounds [(type_identifier) (scoped_type_identifier) (generic_type)] @supertype)) @supertypes
