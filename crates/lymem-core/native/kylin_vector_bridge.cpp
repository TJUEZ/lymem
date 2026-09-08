#include <algorithm>
#include <cstdint>
#include <cstring>
#include <memory>
#include <string>
#include <vector>

#include <kysdk-vector-engine-client/Database.h>

using namespace VectorDB;

namespace {
struct Client {
    std::shared_ptr<Database> db;
    std::string collection;
};

int fail(const Status& status, char* error, size_t capacity) {
    std::string message = std::to_string(static_cast<int>(status.Code())) + ": " + status.Message();
    if (error != nullptr && capacity > 0) {
        std::strncpy(error, message.c_str(), capacity - 1);
        error[capacity - 1] = '\0';
    }
    return -1;
}
}  // namespace

extern "C" void* lymem_kylin_vector_open(const char* collection, int dimension, char* error, size_t capacity) {
    auto client = std::make_unique<Client>();
    client->db = Database::Create();
    client->collection = collection;
    auto status = client->db->Connect(ConnectParam());
    if (!status.IsOk()) {
        fail(status, error, capacity);
        return nullptr;
    }
    bool exists = false;
    status = client->db->HasCollection(client->collection, exists);
    if (!status.IsOk()) {
        exists = false;  // Current Kylin SDK reports collection-not-found as SERVER_FAILED.
    }
    if (!exists) {
        status = client->db->CreateCollection(client->collection, dimension, false, false);
        if (!status.IsOk()) {
            fail(status, error, capacity);
            return nullptr;
        }
    }
    return client.release();
}

extern "C" int lymem_kylin_vector_upsert(void* raw, int64_t id, const float* vector, size_t dimension,
                                           char* error, size_t capacity) {
    auto* client = static_cast<Client*>(raw);
    std::vector<FieldDataPtr> fields{
        std::make_shared<Int64FieldData>(DEFAULT_ID_FIELD_NAME, std::vector<int64_t>{id}),
        std::make_shared<FloatVecFieldData>(DEFAULT_VECTOR_FIELD_NAME,
                                            std::vector<std::vector<float>>{
                                                std::vector<float>(vector, vector + dimension)}),
    };
    DmlResults results;
    auto status = client->db->Upsert(client->collection, fields, results);
    return status.IsOk() ? 0 : fail(status, error, capacity);
}

extern "C" int lymem_kylin_vector_search(void* raw, const float* vector, size_t dimension, size_t top_k,
                                           int64_t* ids, float* scores, size_t capacity, char* error,
                                           size_t error_capacity) {
    auto* client = static_cast<Client*>(raw);
    SearchArguments arguments(client->collection, static_cast<int64_t>(top_k));
    arguments.SetGuaranteeTimestamp(GuaranteeStrongTs());
    arguments.AddTargetVector(DEFAULT_VECTOR_FIELD_NAME, std::vector<float>{vector, vector + dimension});
    SearchResults results;
    auto status = client->db->Search(arguments, results);
    if (!status.IsOk()) {
        return fail(status, error, error_capacity);
    }
    if (results.Results().empty()) {
        return 0;
    }
    const auto& found_ids = results.Results().front().Ids().IntIDArray();
    const auto& found_scores = results.Results().front().Scores();
    const size_t count = std::min({capacity, found_ids.size(), found_scores.size()});
    for (size_t i = 0; i < count; ++i) {
        ids[i] = found_ids[i];
        scores[i] = found_scores[i];
    }
    return static_cast<int>(count);
}

extern "C" int lymem_kylin_vector_delete(void* raw, const int64_t* ids, size_t count, char* error,
                                           size_t capacity) {
    if (count == 0) return 0;
    auto* client = static_cast<Client*>(raw);
    std::string expression = "id in [";
    for (size_t i = 0; i < count; ++i) {
        if (i > 0) expression += ",";
        expression += std::to_string(ids[i]);
    }
    expression += "]";
    DmlResults results;
    auto status = client->db->Delete(client->collection, expression, results);
    return status.IsOk() ? 0 : fail(status, error, capacity);
}

extern "C" void lymem_kylin_vector_close(void* raw) {
    auto* client = static_cast<Client*>(raw);
    if (client == nullptr) return;
    client->db->Disconnect();
    delete client;
}
