#ifndef MESH_CORE_H
#define MESH_CORE_H
/* Opaque state. Serialize all use of a handle, including destruction. */
typedef struct MeshEngine MeshEngine;

/* Returns a newly allocated secret seed. Release with mesh_string_free. */
char *mesh_seed_new(void);

/* Borrows a readable NUL-terminated UTF-8 seed during the call. Invalid input
 * returns NULL; a successful handle is owned by the caller. */
MeshEngine *mesh_engine_new(const char *seed);

/* Borrows exclusive access to a live engine and a readable NUL-terminated JSON
 * request. Returns owned UTF-8 JSON: {ok:true,data:...} or {ok:false,error:"..."}.
 * Text is limited to 256 KiB after reading the C string. Invalid pointers are
 * not made safe by JSON validation. See docs/IOS_RUST.md for the operations. */
char *mesh_engine_call(MeshEngine *engine, const char *request);

/* Frees one owned engine exactly once. NULL is allowed; never use concurrently
 * with mesh_engine_call and never reuse the released handle. */
void mesh_engine_free(MeshEngine *engine);

/* Frees a string from mesh_seed_new or mesh_engine_call exactly once. NULL is
 * allowed. Copy before freeing; do not use a different allocator. */
void mesh_string_free(char *value);
#endif
