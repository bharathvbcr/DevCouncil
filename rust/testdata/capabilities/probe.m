#import <Foundation/Foundation.h>
#import "Helper.h"

@interface Widget : BaseWidget
- (NSString *)render;
@end

@implementation Widget
- (NSString *)render {
    return [Helper help:self.name];
}
@end
