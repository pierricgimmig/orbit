// Copyright (c) 2021 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ORBIT_API_INTERFACE_ORBIT_LEGACY_H_
#define ORBIT_API_INTERFACE_ORBIT_LEGACY_H_

#include "Orbit.h"

#if ORBIT_API_ENABLED

#ifdef __cplusplus
#define ORBIT_ARG_T OrbitArg
#else
#define ORBIT_ARG_T (struct OrbitArg)
#endif // __cplusplus

#define ORBIT_INT_WITH_COLOR(name, value, col) ORBIT_INT(name, value, col)

#define ORBIT_SCOPE_WITH_COLOR(name, col) ORBIT_SCOPE(name, ORBIT_ARG_T{.color=col})
#define ORBIT_SCOPE_WITH_GROUP_ID(name, group_id_) ORBIT_SCOPE(name, ORBIT_ARG_T{.group_id=group_id_})
#define ORBIT_SCOPE_WITH_COLOR_AND_GROUP_ID(name, col, group_id_) ORBIT_SCOPE(name, ORBIT_ARG_T{.group_id=group_id_, .color=col})
#define ORBIT_ASYNC_STRING(...) // TODO
#define ORBIT_ASYNC_STRING_WITH_COLOR(...) // TODO
#define ORBIT_START_WITH_COLOR(name, col) ORBIT_START(name, ORBIT_ARG_T{.color=col})
#define ORBIT_START_WITH_GROUP_ID(name, group_id_) ORBIT_START(name, ORBIT_ARG_T{.group_id=group_id_})
#define ORBIT_START_WITH_COLOR_AND_GROUP_ID(name, col, group_id_) ORBIT_START(name, ORBIT_ARG_T{.group_id=group_id_, .color=col})
#define ORBIT_START_ASYNC_WITH_COLOR(name, id, col) ORBIT_START_ASYNC(name, id, ORBIT_ARG_T{.color=col})

#define ORBIT_INT64_WITH_COLOR(name, value, col) ORBIT_INT64(name, value, ORBIT_ARG_T{.color=col})
#define ORBIT_UINT_WITH_COLOR(name, value, col) ORBIT_UINT(name, value, ORBIT_ARG_T{.color=col})
#define ORBIT_UINT64_WITH_COLOR(name, value, col) ORBIT_UINT64(name, value, ORBIT_ARG_T{.color=col})
#define ORBIT_FLOAT_WITH_COLOR(name, value, col) ORBIT_FLOAT(name, value, ORBIT_ARG_T{.color=col})
#define ORBIT_DOUBLE_WITH_COLOR(name, value, col) ORBIT_DOUBLE(name, value, ORBIT_ARG_T{.color=col})

#else

#define ORBIT_SCOPE_WITH_COLOR(...)
#define ORBIT_SCOPE_WITH_GROUP_ID(...)
#define ORBIT_SCOPE_WITH_COLOR_AND_GROUP_ID(...)
#define ORBIT_ASYNC_STRING(...)
#define ORBIT_ASYNC_STRING_WITH_COLOR(...)
#define ORBIT_START_WITH_COLOR(...)
#define ORBIT_START_WITH_GROUP_ID(...)
#define ORBIT_START_WITH_COLOR_AND_GROUP_ID(...)
#define ORBIT_START_ASYNC_WITH_COLOR(...)
#define ORBIT_INT_WITH_COLOR(...)
#define ORBIT_INT64_WITH_COLOR(...)
#define ORBIT_UINT_WITH_COLOR(...)
#define ORBIT_UINT64_WITH_COLOR(...)
#define ORBIT_FLOAT_WITH_COLOR(...)
#define ORBIT_DOUBLE_WITH_COLOR(...)

#endif  // ORBIT_API_ENABLED

#endif  // ORBIT_API_INTERFACE_ORBIT_LEGACY_H_
