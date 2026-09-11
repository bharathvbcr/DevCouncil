#import <Foundation/Foundation.h>
@interface Service : NSObject
- (void)run;
@end
@implementation Service
- (void)run { NSLog(@"run"); }
@end
