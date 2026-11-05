/* inventor.h: C ABI of inventor-read (inventor.dll / libinventor.so / libinventor.dylib).
 *
 * Strings and buffers returned belong to the document and stay valid until inventor_close.
 * On error a function returns NULL, -1 or false; inventor_last_error() has the message (per
 * thread). No function panics across this boundary. */
#ifndef INVENTOR_H
#define INVENTOR_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct InventorDocument InventorDocument;

/* One triangle mesh: positions in millimetres, unit normals, a triangle list. */
typedef struct InventorMesh {
    const char *name;
    const float *positions;   /* vertex_count * 3 */
    const float *normals;     /* vertex_count * 3 */
    size_t vertex_count;
    const uint32_t *indices;  /* index_count, 3 per triangle */
    size_t index_count;
} InventorMesh;

const char *inventor_last_error(void);
const char *inventor_version(void);

/* .ipt or .iam; path in UTF-8. */
InventorDocument *inventor_open(const char *path);
InventorDocument *inventor_open_bytes(const uint8_t *data, size_t len);
void inventor_close(InventorDocument *doc);

/* JSON: kind, version, iproperties, property_sets, segments, references, warnings. */
const char *inventor_properties(InventorDocument *doc);
/* The PNG thumbnail, unchanged; NULL when there is none. */
const uint8_t *inventor_thumbnail_png(InventorDocument *doc, size_t *len);

/* Meshes: a part's B-rep tessellated (one mesh per body), or an assembly's display cache.
 * The first call does the work. -1 on error (e.g. an assembly without display cache). */
ptrdiff_t inventor_mesh_count(InventorDocument *doc);
bool inventor_mesh(InventorDocument *doc, size_t index, InventorMesh *out);

/* JSON array of warning strings (skipped faces and the like). */
const char *inventor_warnings(InventorDocument *doc);

#ifdef __cplusplus
}
#endif

#endif
