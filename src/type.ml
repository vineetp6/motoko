(* Representation *)

type con = Con.t
type mut = ConstMut | VarMut
type actor = Object | Actor

type width =
  | Width8
  | Width16
  | Width32
  | Width64

type prim =
  | NullT
  | BoolT
  | NatT
  | IntT
  | WordT of width
  | FloatT
  | CharT
  | TextT

type typ =
  | VarT of con * typ list                     (* constructor *)
  | PrimT of prim                              (* primitive *)
  | ObjT of actor * typ_field list             (* object *)
  | ArrayT of mut * typ                        (* array *)
  | OptT of typ                                (* option *)
  | TupT of typ list                           (* tuple *)
  | FuncT of typ_bind list * typ * typ         (* function *)
  | AsyncT of typ                              (* future *)
  | LikeT of typ                               (* expansion *)
  | AnyT                                       (* top *)
(*
  | UnionT of type * typ                       (* union *)
  | AtomT of string                            (* atom *)
*)

and typ_bind = {var : con; bound : typ}
and typ_field = {var : string; typ : typ; mut : mut}

type kind =
  | DefK of typ_bind list * typ
  | ObjK of typ_bind list * actor * typ_field list
  | ParK of typ_bind list * typ


(* Short-hands *)

let unitT = TupT []
let boolT = PrimT BoolT
let intT = PrimT IntT


(* Pretty printing *)

open Printf

let string_of_mut = function
  | VarMut -> "var "
  | ConstMut -> ""

let string_of_width = function
  | Width8 -> "8"
  | Width16 -> "16"
  | Width32 -> "32"
  | Width64 -> "64"

let string_of_prim = function
  | NullT -> "Null"
  | IntT -> "Int"
  | BoolT -> "Bool"
  | FloatT -> "Float"
  | NatT -> "Nat"
  | WordT w -> "Word" ^ string_of_width w
  | CharT -> "Char"
  | TextT -> "Text"

let rec string_of_typ_nullary = function
  | AnyT -> "Any"
  | PrimT p -> string_of_prim p
  | VarT (c, []) -> Con.to_string c
  | VarT (c, ts) ->
    sprintf "%s<%s>"
      (Con.to_string c) (String.concat ", " (List.map string_of_typ ts))
  | TupT ts ->
    sprintf "(%s)" (String.concat ", " (List.map string_of_typ ts))
  | ObjT (Object, fs) ->
    sprintf "{%s}" (String.concat "; " (List.map string_of_typ_field fs))
  | t ->
    sprintf "(%s)" (string_of_typ t)

and string_of_typ t =
  match t with
  | ArrayT (m, t) ->
    sprintf "%s%s[]" (string_of_mut m) (string_of_typ_nullary t)  
  | FuncT (tbs, t1, t2) ->
    sprintf "%s%s -> %s"
      (string_of_typ_binds tbs) (string_of_typ_nullary t1) (string_of_typ t2)
  | OptT t ->
    sprintf "%s?"  (string_of_typ_nullary t)
  | AsyncT t -> 
    sprintf "async %s" (string_of_typ_nullary t)
  | LikeT t -> 
    sprintf "like %s" (string_of_typ_nullary t)
  | ObjT (Actor, fs) ->
    sprintf "actor %s" (string_of_typ_nullary (ObjT (Object, fs)))
  | t -> string_of_typ_nullary t

and string_of_typ_field {var; mut; typ} =
  sprintf "%s : %s%s" var (string_of_mut mut) (string_of_typ typ)

and string_of_typ_bind {var; bound} =
  (* TODO: print bound *)
  Con.to_string var

and string_of_typ_binds = function
  | [] -> ""
  | tbs -> "<" ^ String.concat ", " (List.map string_of_typ_bind tbs) ^ "> "

let string_of_kind = function
  | DefK (tbs, t) ->
    sprintf "= %s%s" (string_of_typ_binds tbs) (string_of_typ t)
  | ObjK (tbs, actor, fs) -> 
    sprintf ":= %s%s" (string_of_typ_binds tbs) (string_of_typ (ObjT (actor, fs)))
  | ParK (tbs, t) -> 
    sprintf ":: %s%s" (string_of_typ_binds tbs) (string_of_typ t) 


(* First-order substitution *)

type subst = typ Con.Env.t

let rec rename_binds sigma = function
  | [] -> sigma, []
  | {var = con; bound}::binds ->
    let con' = Con.fresh (Con.name con) in
    let sigma' = Con.Env.add con (VarT (con', [])) sigma in
    let (rho, binds') = rename_binds sigma' binds in
    rho, {var = con'; bound = subst rho bound}::binds'

and subst sigma = function
  | PrimT p -> PrimT p
  | VarT (c, ts) ->
    (match Con.Env.find_opt c sigma with
    | Some t -> assert (List.length ts = 0); t
    | None -> VarT (c, List.map (subst sigma) ts)
    )
  | ArrayT (m, t) ->
    ArrayT (m, subst sigma t)
  | TupT ts ->
    TupT (List.map (subst sigma) ts)
  | FuncT(ts, t1, t2) ->
    let (rho, ts') = rename_binds sigma ts in
    FuncT (ts', subst rho t1, subst rho t2)
  | OptT t ->
    OptT (subst sigma t)
  | AsyncT t ->
    AsyncT (subst sigma t)
  | LikeT t ->
    LikeT (subst sigma t)
  | ObjT (a, fs) ->
    ObjT (a, subst_fields sigma fs)

and subst_fields sigma fs = 
  List.map (fun {var; mut; typ} -> {var; mut; typ = subst sigma typ}) fs


let make_subst =
  List.fold_left2 (fun sigma t tb -> Con.Env.add tb.var t sigma) Con.Env.empty


(* Normalization *)

let rec normalize kindenv = function
  | VarT (con, ts) as t ->
    (match Con.Env.find_opt con kindenv with
    | Some (DefK (tbs, t)) -> normalize kindenv (subst (make_subst ts tbs) t)
    | Some _ -> t
    | None -> assert false
    )
  | t -> t


(* Environments *)

module Env = Map.Make(String) 

let union env1 env2 = Env.union (fun k v1 v2 -> Some v2) env1 env2
