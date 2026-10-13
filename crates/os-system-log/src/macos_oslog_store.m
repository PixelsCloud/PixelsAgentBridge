#import <Foundation/Foundation.h>
#import <OSLog/OSLog.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef int (*PABOSLogVisit)(const uint8_t *json, size_t length, int64_t unix_us,
                             const char *message, const char *level, void *context);

static void pab_error(char *buffer, size_t capacity, NSString *message) {
    if (capacity > 0) {
        snprintf(buffer, capacity, "%s", message.UTF8String ?: "OSLogStore failed");
    }
}

// A fresh OSLogStore is a snapshot. The Rust side persists an exact row
// fingerprint and searches from the row's date on the next call. Apple's
// OSLogPosition is opaque and cannot be serialized across Executor restarts.
int pab_oslog_store_visit(double start_unix_seconds, PABOSLogVisit visit,
                          void *context, int *exhausted,
                          char *error_buffer, size_t error_capacity) {
    @autoreleasepool {
        *exhausted = 0;
        if (@available(macOS 10.15, *)) {
            NSError *error = nil;
            OSLogStore *store = [OSLogStore localStoreAndReturnError:&error];
            if (store == nil) {
                pab_error(error_buffer, error_capacity, error.localizedDescription);
                return -1;
            }
            NSDate *date = [NSDate dateWithTimeIntervalSince1970:start_unix_seconds];
            OSLogPosition *position = [store positionWithDate:date];
            OSLogEnumerator *entries =
                [store entriesEnumeratorWithOptions:0 position:position
                                            predicate:nil error:&error];
            if (entries == nil) {
                pab_error(error_buffer, error_capacity, error.localizedDescription);
                return -1;
            }
            for (;;) {
                @autoreleasepool {
                    OSLogEntry *entry = [entries nextObject];
                    if (entry == nil) {
                        *exhausted = 1;
                        return 0;
                    }
                    NSString *message = entry.composedMessage ?: @"";
                    NSMutableDictionary *fields = [NSMutableDictionary dictionary];
                    int64_t unix_us = (int64_t)(entry.date.timeIntervalSince1970 * 1000000.0 + 0.5);
                    fields[@"timestampUnixMicros"] = @(unix_us);
                    fields[@"eventMessage"] = message;
                    fields[@"source"] = @"OSLogStore";
                    NSString *level = nil;
                    if ([entry isKindOfClass:[OSLogEntryLog class]]) {
                        OSLogEntryLog *log = (OSLogEntryLog *)entry;
                        switch (log.level) {
                            case OSLogEntryLogLevelDebug: level = @"Debug"; break;
                            case OSLogEntryLogLevelInfo: level = @"Info"; break;
                            case OSLogEntryLogLevelNotice: level = @"Notice"; break;
                            case OSLogEntryLogLevelError: level = @"Error"; break;
                            case OSLogEntryLogLevelFault: level = @"Fault"; break;
                            default: level = @"Undefined"; break;
                        }
                        fields[@"messageType"] = level;
                    }
                    if ([entry conformsToProtocol:@protocol(OSLogEntryWithPayload)]) {
                        id<OSLogEntryWithPayload> payload = (id<OSLogEntryWithPayload>)entry;
                        if (payload.subsystem != nil) fields[@"subsystem"] = payload.subsystem;
                        if (payload.category != nil) fields[@"category"] = payload.category;
                    }
                    if ([entry conformsToProtocol:@protocol(OSLogEntryFromProcess)]) {
                        id<OSLogEntryFromProcess> process = (id<OSLogEntryFromProcess>)entry;
                        fields[@"processID"] = @(process.processIdentifier);
                        fields[@"threadID"] = @(process.threadIdentifier);
                        if (process.process != nil) fields[@"process"] = process.process;
                        if (process.sender != nil) fields[@"sender"] = process.sender;
                    }
                    NSData *json = [NSJSONSerialization dataWithJSONObject:fields
                                           options:NSJSONWritingSortedKeys error:&error];
                    if (json == nil) {
                        pab_error(error_buffer, error_capacity, error.localizedDescription);
                        return -1;
                    }
                    int action = visit(json.bytes, json.length, unix_us,
                                       message.UTF8String, level.UTF8String, context);
                    if (action < 0) {
                        pab_error(error_buffer, error_capacity, @"OSLogStore row rejected");
                        return -1;
                    }
                    if (action == 0) return 0;
                }
            }
        }
        pab_error(error_buffer, error_capacity, @"OSLogStore requires macOS 10.15 or later");
        return -1;
    }
}

int pab_oslog_store_available(char *error_buffer, size_t error_capacity) {
    @autoreleasepool {
        if (@available(macOS 10.15, *)) {
            NSError *error = nil;
            if ([OSLogStore localStoreAndReturnError:&error] != nil) return 1;
            pab_error(error_buffer, error_capacity, error.localizedDescription);
            return 0;
        }
        pab_error(error_buffer, error_capacity, @"OSLogStore requires macOS 10.15 or later");
        return 0;
    }
}
