// Licensed to the LF AI & Data foundation under one
// or more contributor license agreements. See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership. The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License. You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#pragma once

#include <memory>
#include <vector>

#include "types/CollectionSchema.h"
#include "types/ConnectParam.h"
#include "types/Constants.h"
#include "types/DmlResults.h"
#include "types/FieldData.h"
#include "types/IndexDesc.h"
#include "types/QueryArguments.h"
#include "types/QueryResults.h"
#include "types/SearchArguments.h"
#include "types/SearchResults.h"
#include "types/Status.h"

namespace VectorDB {

class Database {
public:
    /**
     * now only support create a MilvusClient instance.
     *
     * @return std::shared_ptr<Database>
     */
    static std::shared_ptr<Database> Create();

    /**
     * Connect to VectorDB server.
     *
     * @param [in] connect_param server address port and authorization
     * @return Status operation successfully or not
     */
    virtual Status Connect(const ConnectParam& connect_param) = 0;

    /**
     * Connect to VectorDB server. Simple way
     *
     * @return Status operation successfully or not
     */
    Status Connect() { return Connect(ConnectParam()); }

    /**
     * Break connections between client and server.
     *
     * @return Status operation successfully or not
     */
    virtual Status Disconnect() = 0;

    /**
     * Create a collection with schema. And create an index on a field.
     * Not load collection data into CPU memory of query node.
     *
     * @param [in] schema schema of the collection
     * @param [in] index_desc the index descriptions and parameters
     * @return Status operation successfully or not
     */
    virtual Status CreateCollection(const CollectionSchema& schema, const IndexDesc& index_desc) = 0;

    /**
     * Create a collection fast way. Create id and vector two fields. And create vector index.
     * Not load collection data into CPU memory of query node.
     *
     * @param [in] collection_name name of the collection
     * @param [in] dim vector dimension
     * @param [in] auto_id open auto_id or not, default is open.
     * @param [in] enable_dynamic_field 如果开启，插入数据时必须要有个字段的类型是动态字段
     * @return Status operation successfully or not
     */
    virtual Status CreateCollection(const std::string& collection_name, int dim, bool auto_id = true,
                                    bool enable_dynamic_field = true) = 0;

    /**
     * Check existence of a collection.
     *
     * @param [in] collection_name name of the collection
     * @param [out] has true: collection exists, false: collection doesn't exist
     * @return Status operation successfully or not
     */
    virtual Status HasCollection(const std::string& collection_name, bool& has) = 0;

    /**
     * Drop a collection, with all its partitions, index and segments.
     *
     * @param [in] collection_name name of the collection
     * @return Status operation successfully or not
     */
    virtual Status DropCollection(const std::string& collection_name) = 0;

    /**
     * Insert entities into a collection.
     *
     * @param [in] collection_name name of the collection
     * @param [in] fields insert data
     * @param [out] results insert results
     * @return Status operation successfully or not
     */
    virtual Status Insert(const std::string& collection_name, const std::vector<FieldDataPtr>& fields,
                          DmlResults& results) = 0;

    /**
     * Delete entities by filtering condition.
     *
     * @param [in] collection_name name of the collection
     * @param [in] expression the expression to filter out entities, currently only support primary key as filtering.
     * For example: "id in [1, 2, 3]"
     * @param [out] results insert results
     * @return Status operation successfully or not
     */
    virtual Status Delete(const std::string& collection_name, const std::string& expression, DmlResults& results) = 0;

    /**
     * Upsert entities into a collection.
     *
     * @param [in] collection_name name of the collection
     * @param [in] fields upsert data
     * @param [out] results upsert results
     * @return Status operation successfully or not
     */
    virtual Status Upsert(const std::string& collection_name, const std::vector<FieldDataPtr>& fields,
                          DmlResults& results) = 0;

    /**
     * Search a collection based on the given parameters and return results.
     *
     * @param [in] arguments search arguments
     * @param [out] results search results
     * @param [in] timeout search timeout in milliseconds
     * @return Status operation successfully or not
     */
    virtual Status Search(const SearchArguments& arguments, SearchResults& results, int timeout = 0) = 0;

    /**
     * Query with a set of criteria, and results in a list of records that match the query exactly.
     *
     * @param [in] arguments query arguments
     * @param [out] results query results
     * @param [in] timeout search timeout in milliseconds
     * @return Status operation successfully or not
     */
    virtual Status Query(const QueryArguments& arguments, QueryResults& results, int timeout = 0) = 0;

    /**
     * Load collection data into CPU memory of query node.
     * now no need to call
     *
     * @param [in] collection_name name of the collection
     * @return Status operation successfully or not
     */
    virtual Status LoadCollection(const std::string& collection_name) = 0;

    /**
     * Release collection data from query node.
     * now no need to call
     *
     * @param [in] collection_name name of the collection
     * @return Status operation successfully or not
     */
    virtual Status ReleaseCollection(const std::string& collection_name) = 0;
};
}  // namespace VectorDB
