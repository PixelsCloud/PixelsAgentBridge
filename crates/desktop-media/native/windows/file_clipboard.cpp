// System file clipboard provider. Only metadata is advertised; IStream reads
// are bounded requests to the authenticated parent, never source path opens.
#include <windows.h>
#include <ole2.h>
#include <shlobj.h>
#include <shldisp.h>
#include <algorithm>
#include <array>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <deque>
#include <functional>
#include <future>
#include <map>
#include <memory>
#include <mutex>
#include <set>
#include <string>
#include <thread>
#include <vector>

namespace {
constexpr ULONG chunk_size = 256 * 1024;
struct Descriptor { wchar_t name[260]; uint64_t size, modified; uint32_t directory; };
struct Request { uint64_t id, stream, batch, offset; uint32_t index, length, end; unsigned char offer[16]; };
struct Pending { Request request{}; std::condition_variable ready; bool done=false, failed=false; std::vector<unsigned char> data; };
struct State {
    std::mutex mutex;
    bool alive=true;
    uint64_t epoch=1;
    uint64_t next=1;
    std::map<uint64_t,std::shared_ptr<Pending>> pending;
    std::deque<Request> events;
    uint64_t sequence() { std::lock_guard<std::mutex> lock(mutex); return next++; }
    void cancel() {
        std::lock_guard<std::mutex> lock(mutex); alive=false;
        for(auto& pair:pending){pair.second->failed=true;pair.second->done=true;pair.second->ready.notify_all();}
        events.clear();
    }
};
struct Offer {
    std::shared_ptr<State> state;
    std::array<unsigned char,16> id{};
    std::vector<Descriptor> files;
    std::atomic<bool> retired{false};
    uint64_t epoch=0;
};
struct Batch {
    std::shared_ptr<Offer> offer;uint64_t id;
    std::atomic<bool> started{false},ended{false};
    explicit Batch(std::shared_ptr<Offer> value):offer(std::move(value)),id(offer->state->sequence()){}
    bool begin(bool new_stream){
        auto s=offer->state;std::lock_guard<std::mutex> lock(s->mutex);
        if(ended||!s->alive||offer->epoch!=s->epoch||offer->retired&&!started)return false;if(started&&!new_stream)return true;
        if(s->events.size()>=28)return false;
        // Some synchronous targets omit EndOperation. A new stream renews an
        // idle source lease only if that offer is still current at the source.
        Request r{};r.stream=id;r.batch=id;r.end=2;memcpy(r.offer,offer->id.data(),16);s->events.push_back(r);started=true;return true;
    }
    void finish() noexcept{try{if(!started||ended.exchange(true))return;auto s=offer->state;std::lock_guard<std::mutex> lock(s->mutex);if(s->alive&&s->events.size()<32){Request r{};r.stream=id;r.batch=id;r.end=3;memcpy(r.offer,offer->id.data(),16);s->events.push_back(r);}}catch(...){} }
    ~Batch(){finish();}
};
struct ReadLease {
    std::shared_ptr<Offer> offer;
    std::shared_ptr<Batch> batch;
    uint64_t id;
    std::atomic<bool> started{false};
    explicit ReadLease(std::shared_ptr<Batch> value):offer(value->offer),batch(std::move(value)),id(offer->state->sequence()){}
    ~ReadLease() noexcept try {
        if(!started)return;
        auto s=offer->state;std::lock_guard<std::mutex> lock(s->mutex);
        if(s->alive && s->events.size()<32){Request r{};r.stream=id;r.batch=batch->id;r.end=1;memcpy(r.offer,offer->id.data(),16);s->events.push_back(r);}
    }catch(...){}
};

class FileStream final:public IStream {
    std::atomic<ULONG> references{1};
    std::shared_ptr<ReadLease> lease;
    ULONG index;
    uint64_t position=0;
    std::mutex position_mutex;
    IUnknown* marshaler=nullptr;
public:
    FileStream(std::shared_ptr<ReadLease> value,ULONG file,uint64_t offset=0):lease(std::move(value)),index(file),position(offset){CoCreateFreeThreadedMarshaler(this,&marshaler);}
    ~FileStream(){if(marshaler)marshaler->Release();}
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid,void** out) override {
        if(!out)return E_POINTER;*out=nullptr;
        if(iid==IID_IUnknown||iid==IID_ISequentialStream||iid==IID_IStream){*out=static_cast<IStream*>(this);AddRef();return S_OK;}
        if(iid==IID_IMarshal&&marshaler)return marshaler->QueryInterface(iid,out);return E_NOINTERFACE;
    }
    ULONG STDMETHODCALLTYPE AddRef() override{return ++references;}
    ULONG STDMETHODCALLTYPE Release() override{auto n=--references;if(!n)delete this;return n;}
    HRESULT STDMETHODCALLTYPE Read(void* destination,ULONG count,ULONG* received) override try {
        if(received)*received=0;if(count&&!destination)return STG_E_INVALIDPOINTER;
        std::lock_guard<std::mutex> position_lock(position_mutex);
        auto offer=lease->offer;auto s=offer->state;
        if(count){if(!lease->batch->begin(!lease->started))return STG_E_REVERTED;lease->started=true;}
        const uint64_t size=offer->files[index].size;ULONG total=0;
        while(total<count && position<size){
            auto p=std::make_shared<Pending>();
            p->request.stream=lease->id;p->request.batch=lease->batch->id;p->request.index=index;p->request.offset=position;
            p->request.length=static_cast<ULONG>((std::min)({uint64_t(count-total),uint64_t(chunk_size),size-position}));
            memcpy(p->request.offer,offer->id.data(),16);
            std::unique_lock<std::mutex> lock(s->mutex);
            if(!s->alive||offer->epoch!=s->epoch)return STG_E_REVERTED;
            if(s->pending.size()>=4||s->events.size()>=32)return HRESULT_FROM_WIN32(ERROR_BUSY);
            p->request.id=s->next++;s->pending.emplace(p->request.id,p);s->events.push_back(p->request);lease->started=true;
            bool ready=p->ready.wait_for(lock,std::chrono::seconds(15),[&]{return p->done||!s->alive;});
            s->pending.erase(p->request.id);
            // Remove timed-out events too: no stale read can open a source later.
            s->events.erase(std::remove_if(s->events.begin(),s->events.end(),[&](const Request& r){return r.id==p->request.id;}),s->events.end());
            if(!ready||!s->alive||p->failed||p->data.size()!=p->request.length)return STG_E_READFAULT;
            memcpy(static_cast<unsigned char*>(destination)+total,p->data.data(),p->data.size());
            total+=static_cast<ULONG>(p->data.size());position+=p->data.size();if(received)*received=total;
        }
        return total==count?S_OK:S_FALSE;
    }catch(...){return E_OUTOFMEMORY;}
    HRESULT STDMETHODCALLTYPE Seek(LARGE_INTEGER delta,DWORD origin,ULARGE_INTEGER* result) override {
        std::lock_guard<std::mutex> lock(position_mutex);uint64_t base;
        if(origin==STREAM_SEEK_SET)base=0;else if(origin==STREAM_SEEK_CUR)base=position;else if(origin==STREAM_SEEK_END)base=lease->offer->files[index].size;else return STG_E_INVALIDFUNCTION;
        uint64_t next;
        if(delta.QuadPart<0){uint64_t magnitude=uint64_t(-(delta.QuadPart+1))+1;if(magnitude>base)return STG_E_INVALIDFUNCTION;next=base-magnitude;}
        else{uint64_t add=uint64_t(delta.QuadPart);if(add>UINT64_MAX-base)return STG_E_INVALIDFUNCTION;next=base+add;}
        position=next;if(result)result->QuadPart=next;return S_OK;
    }
    HRESULT STDMETHODCALLTYPE Stat(STATSTG* stat,DWORD flags) override {
        if(!stat)return E_POINTER;*stat={};auto& f=lease->offer->files[index];stat->type=STGTY_STREAM;stat->cbSize.QuadPart=f.size;stat->grfMode=STGM_READ;
        if(!(flags&STATFLAG_NONAME)){size_t bytes=(wcslen(f.name)+1)*sizeof(wchar_t);stat->pwcsName=static_cast<LPOLESTR>(CoTaskMemAlloc(bytes));if(!stat->pwcsName)return E_OUTOFMEMORY;memcpy(stat->pwcsName,f.name,bytes);}return S_OK;
    }
    HRESULT STDMETHODCALLTYPE Clone(IStream** out) override {if(!out)return E_POINTER;std::lock_guard<std::mutex> lock(position_mutex);*out=new(std::nothrow) FileStream(lease,index,position);return *out?S_OK:E_OUTOFMEMORY;}
    HRESULT STDMETHODCALLTYPE Write(const void*,ULONG,ULONG*) override{return STG_E_ACCESSDENIED;}
    HRESULT STDMETHODCALLTYPE SetSize(ULARGE_INTEGER) override{return STG_E_ACCESSDENIED;}
    HRESULT STDMETHODCALLTYPE CopyTo(IStream* target,ULARGE_INTEGER count,ULARGE_INTEGER* read,ULARGE_INTEGER* written) override {
        if(read)read->QuadPart=0;if(written)written->QuadPart=0;if(!target)return STG_E_INVALIDPOINTER;
        try{std::vector<unsigned char> buffer(chunk_size);uint64_t total=0;
            while(total<count.QuadPart){ULONG received=0,stored=0;ULONG wanted=static_cast<ULONG>((std::min)(uint64_t(chunk_size),count.QuadPart-total));
                HRESULT status=Read(buffer.data(),wanted,&received);if(FAILED(status))return status;
                if(received){HRESULT output=target->Write(buffer.data(),received,&stored);if(read)read->QuadPart+=received;if(written)written->QuadPart+=stored;if(FAILED(output))return output;if(stored!=received)return STG_E_WRITEFAULT;total+=received;}
                if(status==S_FALSE||!received)break;
            }return S_OK;
        }catch(...){return E_OUTOFMEMORY;}
    }
    HRESULT STDMETHODCALLTYPE Commit(DWORD) override{return S_OK;}
    HRESULT STDMETHODCALLTYPE Revert() override{return STG_E_INVALIDFUNCTION;}
    HRESULT STDMETHODCALLTYPE LockRegion(ULARGE_INTEGER,ULARGE_INTEGER,DWORD) override{return STG_E_INVALIDFUNCTION;}
    HRESULT STDMETHODCALLTYPE UnlockRegion(ULARGE_INTEGER,ULARGE_INTEGER,DWORD) override{return STG_E_INVALIDFUNCTION;}
};

class FileObject final:public IDataObject,public IDataObjectAsyncCapability {
    std::atomic<ULONG> references{1};
    std::shared_ptr<Offer> offer;
    IUnknown* marshaler=nullptr;
    std::atomic<bool> asynchronous{true},in_operation{false};
    std::mutex batch_mutex;std::shared_ptr<Batch> current_batch;ULONG operations=0;
    std::shared_ptr<Batch> batch(){std::lock_guard<std::mutex> lock(batch_mutex);if(!current_batch||current_batch->ended)current_batch=std::make_shared<Batch>(offer);return current_batch;}
    CLIPFORMAT descriptors=static_cast<CLIPFORMAT>(RegisterClipboardFormatW(CFSTR_FILEDESCRIPTORW));
    CLIPFORMAT contents=static_cast<CLIPFORMAT>(RegisterClipboardFormatW(CFSTR_FILECONTENTS));
    CLIPFORMAT effect=static_cast<CLIPFORMAT>(RegisterClipboardFormatW(CFSTR_PREFERREDDROPEFFECT));
public:
    explicit FileObject(std::shared_ptr<Offer> value):offer(std::move(value)){CoCreateFreeThreadedMarshaler(static_cast<IDataObject*>(this),&marshaler);}
    ~FileObject(){if(marshaler)marshaler->Release();}
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid,void** out) override {
        if(!out)return E_POINTER;*out=nullptr;
        if(iid==IID_IUnknown||iid==IID_IDataObject)*out=static_cast<IDataObject*>(this);
        else if(iid==IID_IDataObjectAsyncCapability)*out=static_cast<IDataObjectAsyncCapability*>(this);
        else if(iid==IID_IMarshal&&marshaler)return marshaler->QueryInterface(iid,out);
        else return E_NOINTERFACE;AddRef();return S_OK;
    }
    ULONG STDMETHODCALLTYPE AddRef() override{return ++references;}
    ULONG STDMETHODCALLTYPE Release() override{auto n=--references;if(!n)delete this;return n;}
    HRESULT STDMETHODCALLTYPE QueryGetData(FORMATETC* format) override {
        if(!format)return E_POINTER;if(format->dwAspect!=DVASPECT_CONTENT)return DV_E_DVASPECT;
        if(format->cfFormat==contents){if(!(format->tymed&TYMED_ISTREAM))return DV_E_TYMED;if(format->lindex<0||size_t(format->lindex)>=offer->files.size()||offer->files[format->lindex].directory)return DV_E_LINDEX;return S_OK;}
        if(format->cfFormat==descriptors||format->cfFormat==effect)return format->tymed&TYMED_HGLOBAL?S_OK:DV_E_TYMED;
        return DV_E_FORMATETC;
    }
    HRESULT STDMETHODCALLTYPE GetData(FORMATETC* format,STGMEDIUM* medium) override try {
        if(!medium)return E_POINTER;*medium={};HRESULT checked=QueryGetData(format);if(FAILED(checked))return checked;
        if(format->cfFormat==contents){auto job=batch();if(offer->retired&&!job->started)return STG_E_REVERTED;medium->tymed=TYMED_ISTREAM;medium->pstm=new(std::nothrow) FileStream(std::make_shared<ReadLease>(job),format->lindex);return medium->pstm?S_OK:E_OUTOFMEMORY;}
        size_t size=format->cfFormat==effect?sizeof(DWORD):sizeof(UINT)+offer->files.size()*sizeof(FILEDESCRIPTORW);
        HGLOBAL allocation=GlobalAlloc(GMEM_MOVEABLE|GMEM_ZEROINIT,size);if(!allocation)return E_OUTOFMEMORY;
        void* memory=GlobalLock(allocation);if(!memory){GlobalFree(allocation);return E_OUTOFMEMORY;}
        if(format->cfFormat==effect)*static_cast<DWORD*>(memory)=DROPEFFECT_COPY;
        else{auto* group=static_cast<FILEGROUPDESCRIPTORW*>(memory);group->cItems=static_cast<UINT>(offer->files.size());
            for(size_t i=0;i<offer->files.size();++i){const auto& source=offer->files[i];auto& file=group->fgd[i];
                file.dwFlags=FD_ATTRIBUTES|FD_PROGRESSUI|FD_UNICODE|FD_WRITESTIME;
                file.dwFileAttributes=source.directory?FILE_ATTRIBUTE_DIRECTORY:FILE_ATTRIBUTE_NORMAL;
                if(!source.directory){file.dwFlags|=FD_FILESIZE;file.nFileSizeHigh=DWORD(source.size>>32);file.nFileSizeLow=DWORD(source.size);}
                file.ftLastWriteTime.dwHighDateTime=DWORD(source.modified>>32);file.ftLastWriteTime.dwLowDateTime=DWORD(source.modified);memcpy(file.cFileName,source.name,sizeof(source.name));}}
        GlobalUnlock(allocation);medium->tymed=TYMED_HGLOBAL;medium->hGlobal=allocation;return S_OK;
    }catch(...){return E_OUTOFMEMORY;}
    HRESULT STDMETHODCALLTYPE EnumFormatEtc(DWORD direction,IEnumFORMATETC** out) override {
        if(direction!=DATADIR_GET)return E_NOTIMPL;
        FORMATETC formats[]={{descriptors,nullptr,DVASPECT_CONTENT,-1,TYMED_HGLOBAL},{contents,nullptr,DVASPECT_CONTENT,-1,TYMED_ISTREAM},{effect,nullptr,DVASPECT_CONTENT,-1,TYMED_HGLOBAL}};
        return SHCreateStdEnumFmtEtc(3,formats,out);
    }
    HRESULT STDMETHODCALLTYPE SetData(FORMATETC* format,STGMEDIUM* medium,BOOL release) override {
        if(!format||!medium)return E_POINTER;
        auto performed=RegisterClipboardFormatW(CFSTR_PERFORMEDDROPEFFECT),succeeded=RegisterClipboardFormatW(CFSTR_PASTESUCCEEDED);
        if(format->cfFormat!=performed&&format->cfFormat!=succeeded)return E_NOTIMPL;
        // Outcome notifications are advisory. No callback ever deletes sources.
        if(release)ReleaseStgMedium(medium);return S_OK;
    }
    HRESULT STDMETHODCALLTYPE GetDataHere(FORMATETC*,STGMEDIUM*) override{return E_NOTIMPL;}
    HRESULT STDMETHODCALLTYPE GetCanonicalFormatEtc(FORMATETC*,FORMATETC* out) override{if(out)out->ptd=nullptr;return E_NOTIMPL;}
    HRESULT STDMETHODCALLTYPE DAdvise(FORMATETC*,DWORD,IAdviseSink*,DWORD*) override{return OLE_E_ADVISENOTSUPPORTED;}
    HRESULT STDMETHODCALLTYPE DUnadvise(DWORD) override{return OLE_E_ADVISENOTSUPPORTED;}
    HRESULT STDMETHODCALLTYPE EnumDAdvise(IEnumSTATDATA**) override{return OLE_E_ADVISENOTSUPPORTED;}
    HRESULT STDMETHODCALLTYPE SetAsyncMode(BOOL value) override{asynchronous=value!=FALSE;return S_OK;}
    HRESULT STDMETHODCALLTYPE GetAsyncMode(BOOL* value) override{if(!value)return E_POINTER;*value=asynchronous?TRUE:FALSE;return S_OK;}
    HRESULT STDMETHODCALLTYPE StartOperation(IBindCtx*) override{std::lock_guard<std::mutex> lock(batch_mutex);++operations;in_operation=true;return S_OK;}
    HRESULT STDMETHODCALLTYPE InOperation(BOOL* value) override{if(!value)return E_POINTER;*value=in_operation?TRUE:FALSE;return S_OK;}
    HRESULT STDMETHODCALLTYPE EndOperation(HRESULT,IBindCtx*,DWORD) override{std::lock_guard<std::mutex> lock(batch_mutex);if(operations)--operations;if(!operations){in_operation=false;if(current_batch)current_batch->finish();current_batch.reset();}return S_OK;}
};

struct Context {
    std::shared_ptr<State> state=std::make_shared<State>();
    std::thread thread;DWORD thread_id=0;
    std::mutex mutex;std::deque<std::function<void()>> commands;
    FileObject* object=nullptr;std::shared_ptr<Offer> offer;
    Context(){
        std::promise<HRESULT> ready;auto future=ready.get_future();
        thread=std::thread([this,&ready]{HRESULT status=OleInitialize(nullptr);thread_id=GetCurrentThreadId();MSG msg;PeekMessageW(&msg,nullptr,0,0,PM_NOREMOVE);ready.set_value(status);
            if(FAILED(status))return;
            while(GetMessageW(&msg,nullptr,0,0)>0){
                if(!msg.hwnd&&msg.message==WM_APP){std::deque<std::function<void()>> queue;{std::lock_guard<std::mutex> lock(mutex);queue.swap(commands);}for(auto& command:queue)command();}
                else{TranslateMessage(&msg);DispatchMessageW(&msg);}}
            if(object){if(OleIsCurrentClipboard(object)==S_OK)OleSetClipboard(nullptr);object->Release();object=nullptr;}OleUninitialize();});
        HRESULT status=future.get();if(FAILED(status)){thread.join();throw status;}
    }
    ~Context(){state->cancel();PostThreadMessageW(thread_id,WM_QUIT,0,0);if(thread.joinable())thread.join();}
    template<class F> auto invoke(F function)->decltype(function()) {
        using R=decltype(function());auto task=std::make_shared<std::packaged_task<R()>>(std::move(function));auto result=task->get_future();
        {std::lock_guard<std::mutex> lock(mutex);commands.push_back([task]{(*task)();});}
        if(!PostThreadMessageW(thread_id,WM_APP,0,0))throw E_FAIL;
        return result.get();
    }
};
}

extern "C" void* pab_fc_new(){try{return new Context();}catch(...){return nullptr;}}
extern "C" void pab_fc_free(void* opaque){delete static_cast<Context*>(opaque);}
extern "C" int pab_fc_sources(void* opaque,wchar_t* paths,uint32_t capacity,uint64_t* sequence){
    try{auto* c=static_cast<Context*>(opaque);return c->invoke([=]()->int{
        auto previous=*sequence;*sequence=GetClipboardSequenceNumber();if(previous==*sequence)return -5;
        if(c->object&&OleIsCurrentClipboard(c->object)==S_OK)return -2;
        if(c->offer)c->offer->retired=true;
        if(c->object){c->object->Release();c->object=nullptr;}c->offer.reset();
        if(!IsClipboardFormatAvailable(CF_HDROP))return 0;
        if(!OpenClipboard(nullptr))return -1;
        struct Close{~Close(){CloseClipboard();}} close;
        auto effect=RegisterClipboardFormatW(CFSTR_PREFERREDDROPEFFECT);
        if(auto h=GetClipboardData(effect)){if(GlobalSize(h)>=sizeof(DWORD)){auto* p=static_cast<DWORD*>(GlobalLock(h));if(p){bool cut=(*p&DROPEFFECT_MOVE)!=0;GlobalUnlock(h);if(cut)return -3;}}}
        auto drop=static_cast<HDROP>(GetClipboardData(CF_HDROP));if(!drop)return -1;
        UINT count=DragQueryFileW(drop,0xffffffff,nullptr,0);if(!count||count>4096)return -4;
        uint32_t used=0;
        for(UINT i=0;i<count;++i){UINT length=DragQueryFileW(drop,i,nullptr,0);if(!length||length>32767||used+length+2>capacity)return -4;
            if(DragQueryFileW(drop,i,paths+used,length+1)!=length)return -1;used+=length+1;}
        paths[used++]=0;if(*sequence!=GetClipboardSequenceNumber())return -1;return static_cast<int>(used);
    });}catch(...){return -1;}
}
extern "C" int pab_fc_publish(void* opaque,const unsigned char* id,const Descriptor* files,uint32_t count,uint64_t expected){
    try{if(!count||count>10000)return -1;auto* c=static_cast<Context*>(opaque);auto offer=std::make_shared<Offer>();offer->state=c->state;{std::lock_guard<std::mutex> lock(c->state->mutex);offer->epoch=c->state->epoch;}memcpy(offer->id.data(),id,16);offer->files.assign(files,files+count);
        struct Less{bool operator()(const std::wstring& a,const std::wstring& b)const{return CompareStringOrdinal(a.data(),static_cast<int>(a.size()),b.data(),static_cast<int>(b.size()),TRUE)==CSTR_LESS_THAN;}};
        std::set<std::wstring,Less> names;for(const auto& file:offer->files){if(file.name[259]!=0||!names.insert(file.name).second)return -1;}
        return c->invoke([=]()->int{if(expected!=UINT64_MAX&&expected!=GetClipboardSequenceNumber())return -2;
            auto* object=new FileObject(offer);HRESULT status=OleSetClipboard(object);if(FAILED(status)){object->Release();return -1;}
            if(c->offer)c->offer->retired=true;if(c->object)c->object->Release();c->offer=offer;c->object=object;return 0;});
    }catch(...){return -1;}
}
extern "C" int pab_fc_poll(void* opaque,Request* request){
    try{auto s=static_cast<Context*>(opaque)->state;std::lock_guard<std::mutex> lock(s->mutex);if(s->events.empty())return 0;*request=s->events.front();s->events.pop_front();return 1;}catch(...){return -1;}
}
extern "C" int pab_fc_reply(void* opaque,uint64_t id,const unsigned char* bytes,uint32_t length,int failed){
    try{if(length>chunk_size)return -1;auto s=static_cast<Context*>(opaque)->state;std::lock_guard<std::mutex> lock(s->mutex);auto it=s->pending.find(id);if(it==s->pending.end())return 0;
        auto p=it->second;p->failed=failed!=0||length!=p->request.length;if(!p->failed)p->data.assign(bytes,bytes+length);p->done=true;p->ready.notify_all();return 0;
    }catch(...){return -1;}
}
extern "C" void pab_fc_clear(void* opaque){
    try{auto* c=static_cast<Context*>(opaque);
        {std::lock_guard<std::mutex> lock(c->state->mutex);++c->state->epoch;for(auto& pair:c->state->pending){pair.second->failed=true;pair.second->done=true;pair.second->ready.notify_all();}c->state->events.clear();}
        c->invoke([=]{if(c->offer)c->offer->retired=true;if(c->object){if(OleIsCurrentClipboard(c->object)==S_OK)OleSetClipboard(nullptr);c->object->Release();c->object=nullptr;}c->offer.reset();});}catch(...){}
}
extern "C" void pab_fc_withdraw(void* opaque,const unsigned char* id){
    try{auto* c=static_cast<Context*>(opaque);std::array<unsigned char,16> offer;memcpy(offer.data(),id,16);
        c->invoke([=]{if(c->offer&&c->offer->id==offer){c->offer->retired=true;if(c->object){if(OleIsCurrentClipboard(c->object)==S_OK)OleSetClipboard(nullptr);c->object->Release();c->object=nullptr;}c->offer.reset();}});
    }catch(...){}
}
