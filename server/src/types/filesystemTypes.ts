export interface RequestWithUser extends Express.Request {
    user?: any; // You can replace 'any' with a more specific type if needed
}

export interface ResponseData {
    success: boolean;
    message: string;
    data?: any; // You can replace 'any' with a more specific type if needed
}